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
; Candidacy is ledger-gated upstream (capture_http_client_call_arm): these
; anchors are evaluated only in a file referencing `net::http`, so a same-shaped
; `cache.Get("k")` elsewhere is never scanned at all (FR-WS-08 shared
; negative-case fixture contract, case 1).
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
;     captured text with no row is dropped, so `http.Get`/`c.Head` would each
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
; discriminators keep them apart:
;
;   * verb-as-method-name — `net/http`'s verb functions take the path either
;     ALONE (`http.Get(url)`, `c.Head(url)`) or followed by a content-type
;     STRING (`http.Post(url, "application/json", body)`). A registration always
;     passes a handler — an identifier or func literal — after the path, so
;     "path alone, or path then a string" excludes every `r.GET("/users", h)` /
;     `app.Get("/users", h)` form. A text predicate cannot help here: the only
;     text available is the receiver name (`c`/`client` vs `r`/`router`), the
;     same fragile heuristic CR-110 is currently cleaning up after.
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

; ── Verb-as-method-name, path is the sole argument ──────────────────────────
; `http.Get("/p")`, `c.Head("/p")`, and their runtime-composed forms
; (`http.Get(url)`, `http.Get(fmt.Sprintf(…))`) — one pattern, because
; `static_string_literal` is what separates a static path from a composed one.
; A composed path still yields a SITE, so the arm classifies it base-url-runtime
; instead of never seeing it; the normalizer then emits no reference. (That
; classification is not yet observable in any output — nothing consumes
; `UnboundReason` outside tests — so this is forward parity with the Rust arm,
; not a live coverage guarantee.)
(call_expression
  function: (selector_expression
    field: (field_identifier) @invoke.http.method)
  arguments: (argument_list
    .
    (_) @invoke.http.arg
    .))

; ── Verb-as-method-name, path followed by a content-type string ─────────────
; `http.Post("/p", "application/json", body)`. The second argument must be a
; string literal, which a route handler never is.
(call_expression
  function: (selector_expression
    field: (field_identifier) @invoke.http.method)
  arguments: (argument_list
    .
    (_) @invoke.http.arg
    .
    [(interpreted_string_literal) (raw_string_literal)]))

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
