; Go HTTP client-call capture (S-345, capability = "invocations", FR-WS-08,
; CR-108). `net/http` — the one client library FR-WS-08's normative row names.
;
; Capture contract (extract::collect_invocation_sites): exactly two names, and
; the code — not the query — makes every judgment.
;
;   @invoke.http.method — a node whose TEXT is the HTTP verb. Kept only when
;                         `is_http_method` recognises it (case-insensitive).
;   @invoke.http.arg    — the request-path node. Kept as a `"METHOD /template"`
;                         reference only when `static_string_literal` reads a
;                         static literal out of it; anything else is refused as
;                         base-url-runtime and never approximately matched
;                         (NFR-RA-05).
;
; A `@_`-prefixed capture is ignored by the dispatch (its match arm falls
; through), so it is free to use for predicate operands.
;
; ── Candidacy: the RECEIVER, not the file (S-402, CR-126, FR-WS-08 AC5) ─────
;
; Candidacy is ledger-gated upstream (capture_http_client_call_arm): these
; anchors are evaluated only in a file referencing `net::http`. That gate is a
; cheap pre-filter and nothing more. Until S-402 this header claimed it was the
; whole of FR-WS-08's negative case 1 — "a same-shaped `cache.Get("k")`
; ELSEWHERE is never scanned at all" — and **elsewhere** is the word that
; failed. The assumption holds outside the file and fails inside it, and it
; fails hardest in the one member shape where Go HTTP analysis matters: an HTTP
; gateway, whose every file imports `net/http` and calls `.Get(` on request
; headers and query strings. Measured on the reference estate's Go member
; (`hermodr-mirror`, 2026-09-13): `r.Header.Get(k)`, `r.URL.Query().Get("idp")`
; and `mapping.Get("authorize.claims")` were **26 of 37** captured sites, none
; an HTTP call — a silently inflated denominator under every egress figure
; (NFR-RA-05, FR-WS-05).
;
; So the decision is made on the RECEIVER, as S-375 made it for Java. The rule
; needs no receiver TYPING: it is a receiver-NAME boundary rule over FR-WS-08's
; normative Go row (`net/http`, the stdlib client and nothing else), spelled as
; the two things that row can be named by:
;
;   * `http` — exactly. The package qualifier that `import "net/http"` binds,
;     which is the whole of `http.Get` / `http.Post`. An aliased import
;     (`import nethttp "net/http"`) falls outside it and is honestly uncaptured,
;     the safe direction — the same posture the constructor anchor below takes.
;   * a name derived from the client TYPE `http.Client`: one STARTING with the
;     type-derived token `httpClient` (optional `_` prefix, optional
;     camel/digit-boundary suffix — `httpClient`, `_httpClient`,
;     `httpClientV2`), one ENDING in the type-derived `HttpClient`/`HTTPClient`
;     (`apiHttpClient`, `usersHTTPClient`), the stdlib package var
;     `DefaultClient`, or the bare word `client` WHOLE.
;     The receiver is read as a bare identifier OR as the FIELD of one selector
;     level down, so `http.DefaultClient.Get(url)` and `s.httpClient.Get(url)`
;     are both admitted while `r.Header.Get(k)` is not — `Header` carries no
;     client token. `r.URL.Query().Get("idp")` matches neither shape at all: its
;     receiver is a call, not a name.
;
; A bare `Client` SUFFIX is deliberately NOT in the vocabulary, and the bare word
; `client` is admitted only WHOLE. This is the Java rule's sharpest line, adopted
; here for the reason `ff257427` recorded when S-375's review added it: the
; suffix names every client protocol in existence, and `cacheClient.Get("/x")`,
; `zkClient.Delete("/config/orders")` and `redisClient.Get("/config/features")`
; each clear the verb and absolute-path filters in ordinary Go — reopening
; CR-110's fabrication class on the consumer side (NFR-RA-05). `clientCache`,
; `clientRegistry` and `clientStore` are the same object spelled the other way
; round, which is why the generic word is whole-only. S-402's first cut admitted
; all six and this review narrowed it; on the reference estate the narrowing
; costs ZERO sites, because every genuine Go receiver there is a bare `client`.
;
; It is a BOUNDARY rule, never a substring test: `clientcache`, `clientCache` and
; `routerClientele` are all refused. Pinned on both edges by
; `go_invocations.rs::the_receiver_rule_is_a_boundary_rule_over_the_normative_go_row`.
;
; The price, and it is Java's price too: a generically-named wrapper
; (`apiClient`, `ordersClient`) stays a stated under-capture ceiling, as does a
; bare package-level `Client`. Under-capture is the safe direction.
;
; Two ceilings follow from it, both stated rather than worked around (ADR-54):
;
;   * A SINGLE-LETTER receiver — `c.Get("/p")` on a `*http.Client` — is refused,
;     because `c` is equally Gin's conventional `*gin.Context` receiver and
;     `c.Get("user")` there is a context lookup: the very shape this rule
;     removes. A name rule cannot separate them, and under-capture is the safe
;     direction (NFR-RA-05). Pinned by
;     `a_single_letter_client_receiver_is_a_stated_ceiling`.
;   * A non-HTTP collaborator SPELLED like a client (`client.Get("/admin/users")`
;     on a cache) still captures inside a gate-admitted file. That residual is
;     narrow and named, where the blanket file-grained one was not. Pinned as the
;     positive control of `a_route_shaped_get_outside_a_net_http_file_is_not_captured`.
;   * A receiver that is not a NAME — a call, an index, a composite literal, a
;     parenthesized expression (`s.Client().Get(url)`, `NewClient().Get(url)`,
;     `clients[0].Get(url)`, `(&http.Client{}).Get(url)`) — is refused by the
;     operand alternation. These were captured before S-402 and are genuine
;     `net/http` shapes, so the narrowing is recorded rather than silent; on the
;     reference estate it costs zero sites (every site it newly refuses there is
;     a `.Header.Get` / `*Store.Get`, none an outbound call). `extract::
;     capture_http_client_call_arm` enumerates the same class as "a chained
;     receiver".
;
; ── The constructor-argument anchor (S-345's decision, consumed by S-346) ────
;
; Go is the first language whose dominant idiom puts the verb in an ARGUMENT
; rather than in the method name: `http.NewRequest("GET", "/p", body)` followed
; by `client.Do(req)`. The existing two-name vocabulary expresses this with NO
; capture-vocabulary change, because `@invoke.http.method` is defined by the
; captured node's *text*, not by its grammatical role — nothing requires it to
; be a method identifier. The one subtlety is that the verb node must yield bare
; `GET`, and tree-sitter-go's `interpreted_string_literal` spans its quotes, so
; the METHOD capture lands on the literal's CONTENT child.
;
; The PATH capture does the opposite — it takes the whole literal — because
; `static_string_literal` unquotes it, and handing that function an
; already-unquoted content child sends it down a fallback that trims quote and
; `#` characters, silently turning `"/tag/#"` into `/tag/`. Whole literal for
; the path, content child for the verb; the asymmetry is deliberate.
;
; A verb held in a NAMED CONSTANT (`http.NewRequest(http.MethodGet, …)`) is a
; stated ceiling, not a capture: the only texts available are `http.MethodGet`
; and `MethodGet`, and `is_http_method` speaks bare verbs.
;
; When this was written the normalizer it would need did not exist, and this
; paragraph deferred it as "a logos-core change, stated against NFR-MA-01". That
; is no longer true: S-346 landed `[invocation_methods]` — per-plugin descriptor
; data, read by `extract::normalize_invocation_method`, costing this language no
; core change at all — for C#'s `HttpMethod.Get`, which is the same shape. The
; ceiling stands anyway, for a DIFFERENT and narrower reason, and it is recorded
; here rather than left to a reader to rediscover:
;
;   * a table alone cannot reach it. The constructor pattern below binds
;     `@invoke.http.method` to an `interpreted_string_literal_content`, and
;     `http.MethodGet` is a `selector_expression` — no text is captured for a
;     row to normalize. Lifting it is a table PLUS a query pattern, a Go-side
;     decision (the c-sharp query header says the same, from the other end).
;   * and declaring any row opts this language into the table's FILTER half: a
;     captured text with no row is dropped, so `http.Get`/`client.Head` would each
;     need an identity row the pass-through gives them for free today.
;
; Measured cost of leaving it: on `pec-services`' `hermodr-mirror`, 23 of 24
; `http.NewRequest` sites carry a runtime variable as the URL and are refused on
; the path regardless of whether the verb resolves (see
; `go_invocations.rs::a_named_constant_verb_is_a_stated_ceiling_not_a_capture`).
;
; ── Why a registration is not a call ────────────────────────────────────────
;
; A provider's own route registration is syntactically a client call: Gin's
; `r.GET("/users", h)` is `<receiver>.<VERB>(<path literal>, …)`, and
; `r.Handle("GET", "/users", h)` is a verb/path argument pair. A Gin service
; almost always imports `net/http` too (for `http.StatusOK`), so the ledger gate
; does not separate them; nor does a test file's `httptest.NewRequest("GET",
; "/users", nil)`, which builds an INBOUND request for a handler test. Capturing
; any of these turns a provider's own surface into phantom outbound calls that
; bind other members' routes — a fabricated cross-service edge (NFR-RA-05). Two
; ARGUMENT-SHAPE discriminators keep them apart, independently of the receiver
; rule above — a registration is refused twice over, which is deliberate:
;
;   * verb-as-method-name — `net/http`'s verb functions take the path either
;     ALONE (`http.Get(url)`, `client.Head(url)`) or followed by a content-type
;     STRING (`http.Post(url, "application/json", body)`). A registration always
;     passes a handler — an identifier or func literal — after the path, so
;     "path alone, or path then a string" excludes every `r.GET("/users", h)` /
;     `app.Get("/users", h)` form.
;
;     This paragraph used to close by asserting that a text predicate "cannot
;     help here", the receiver name (`c`/`client` vs `r`/`router`) being the
;     fragile heuristic CR-110 was cleaning up after. S-402 retires that claim —
;     see Candidacy above for why a boundary rule over a normative row is not
;     that heuristic. The arity rule stays because it is independent: `r` is
;     refused by the receiver rule, but a router spelled `httpClientRouter`
;     would not be.
;
;   * verb-as-constructor-argument — the callee is pinned BY NAME to the two
;     `net/http` constructors. Nothing structural distinguishes them: an earlier
;     draft required a value position, on the theory that a registration
;     discards its result — but Gin's `Handle` returns `IRoutes` and Echo's
;     `Add` returns `*Route`, so `routes := r.Handle("GET", "/x", h)` defeated
;     it, as did `httptest.NewRequest`, and an unpinned adjacent (string,string)
;     pair also captured `exec.Command("curl", "-X", "GET", "/v1/ping")`.
;     `#eq?`/`#any-of?` predicates ARE evaluated on this path — `cursor.matches`
;     filters through tree-sitter's `satisfies_text_predicates`, which is what
;     `plugins/rust/queries/brokers.scm` already relies on — so the callee is
;     named directly. An aliased import (`import nethttp "net/http"`) falls
;     outside the predicate and is honestly uncaptured, the safe direction.
;
; Deliberately NOT captured: `client.Do(req)` — the verb and path live on the
; `http.NewRequest` that built `req`, and joining them needs dataflow the
; extractor does not have. `http.PostForm` — `PostForm` is not one of
; `is_http_method`'s bare verbs, so no query can reach it; a stated ceiling.
;
; Droppable on disk at `.logos/plugins/go/queries/invocations.scm` (FR-PL-04,
; FR-PL-05).

; ── Verb-as-method-name ─────────────────────────────────────────────────────
; `http.Get("/p")`, `client.Head("/p")` and their runtime-composed forms
; (`http.Get(url)`, `http.Get(fmt.Sprintf(…))`), plus the content-type form
; `http.Post("/p", "application/json", body)`. A composed path still yields a
; SITE, so the arm classifies it base-url-runtime instead of never seeing it;
; the normalizer then emits no reference. (That classification is not yet
; observable in any output — nothing consumes `UnboundReason` outside tests — so
; this is forward parity with the Rust arm, not a live coverage guarantee.)
;
; ONE pattern, not two, with the two argument shapes as a node alternation. The
; receiver rule therefore has a SINGLE copy. S-375's first cut spelled the guard
; twice and its review had to collapse it (`ff257427`): "reverting either copy
; alone re-admitted `this.perms.get(…)` with the whole suite green". S-402's
; first cut reproduced that exact state here, and deleting the second copy left
; all 24 Go fixtures green while `mapping.Get("/authorize/claims",
; "application/json")` fabricated an outbound edge. One copy, or the test cannot
; hold it.
(call_expression
  function: (selector_expression
    operand: [
      (identifier) @_recv
      (selector_expression field: (field_identifier) @_recv)
    ]
    field: (field_identifier) @invoke.http.method)
  arguments: [
    ; `net/http`'s verb functions take the path ALONE …
    (argument_list
      .
      (_) @invoke.http.arg
      .)
    ; … or followed by a content-type STRING, which a route handler never is.
    (argument_list
      .
      (_) @invoke.http.arg
      .
      [(interpreted_string_literal) (raw_string_literal)])
  ]
  (#match? @_recv "^http$|^client$|^DefaultClient$|^_?httpClient([A-Z0-9_][A-Za-z0-9_]*)?$|^_?[A-Za-z][A-Za-z0-9_]*(HttpClient|HTTPClient)$"))

; ── Verb-as-constructor-argument ────────────────────────────────────────────
; `http.NewRequest("GET", "/p", body)` and `http.NewRequestWithContext(ctx,
; "GET", "/p", body)` — one pattern, because the absent leading anchor matches
; the adjacent verb/path pair wherever it sits. The pair must be ADJACENT, which
; keeps `http.Post("/p", "application/json", …)` from reading `/p` as the verb
; (`is_http_method` rejects it in any case).
(call_expression
  function: (selector_expression
    operand: (identifier) @_pkg
    field: (field_identifier) @_fn)
  arguments: (argument_list
    (interpreted_string_literal
      .
      (interpreted_string_literal_content) @invoke.http.method
      .)
    .
    (_) @invoke.http.arg)
  (#eq? @_pkg "http")
  (#any-of? @_fn "NewRequest" "NewRequestWithContext"))
