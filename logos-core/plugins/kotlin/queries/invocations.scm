; Kotlin HTTP client-call capture (S-342, CR-108, capability = "invocations",
; FR-WS-08).
;
; Kotlin binds the SAME JVM client APIs as Java — RestClient, WebClient,
; RestTemplate and `java.net.http.HttpClient` are FR-WS-08's whole normative
; Kotlin row — so the *idiom* row is identical and the emitted keys are
; byte-identical to Java's for equivalent source. The GRAMMAR is not: this file
; is written against tree-sitter-kotlin-ng 1.1's own node shapes, established
; from real parse trees before a line of it was written (S-342's explicit
; acceptance criterion; the recorded trees are in the implementation notes).
; Nothing here was adapted by eye from `plugins/java/queries/invocations.scm`.
;
; Capture contract (`extract::collect_invocation_sites`): exactly two names, and
; the code — not the query — makes every judgment.
;
;   @invoke.http.method — a node whose TEXT is the HTTP verb (`get`, `GET`,
;                         `post`, …). Kept only when `is_http_method`
;                         recognises it.
;   @invoke.http.arg    — the node holding the request path. Kept as a
;                         `"METHOD /template"` reference only when
;                         `static_string_literal` reads a static literal out of
;                         it; anything else is refused as base-url-runtime and
;                         never approximately matched (NFR-RA-05).
;
; A `@_`-prefixed capture is ignored by the dispatch (its match arm falls
; through), so it is free to use for predicate operands.
;
; Candidacy is ledger-gated upstream (`capture_http_client_call_arm`): these
; anchors are evaluated only in a file whose reference ledger names one of this
; plugin's own `http_client_detectors` rows, so a same-shaped `cache.get("k")`
; elsewhere is never scanned at all (FR-WS-08 shared negative-case fixture
; contract, case 1). Every row is a multi-segment package prefix, never a bare
; identifier, so the gate stays independent evidence (see the dispatch rustdoc).
;
; ── The four Kotlin grammar facts this file is built on ─────────────────────
;
; 1. There is no `method_invocation` and there are NO FIELDS. A receiver call is
;    `(call_expression (navigation_expression <receiver> (identifier)) (value_arguments))`
;    — the member name is the navigation's LAST child, and every constraint here
;    is therefore positional (`.` anchors), never `object:` / `name:`.
;
; 2. An argument is wrapped in `value_argument`, and a NAMED argument puts the
;    parameter name first: `f(url = "/p")` is
;    `(value_argument (identifier) (string_literal))` while `f("/p")` is
;    `(value_argument (string_literal))`. Binding the argument to the
;    value_argument's LAST child (`(value_argument (_) @… .)`) covers both forms
;    with one pattern — which is why the named-argument idiom is captured rather
;    than refused, and why it does not need a pattern of its own.
;
; 3. STRING INTERPOLATION IS NOT MODELLED for the bare `$name` form. This is
;    the one place tree-sitter-kotlin-ng's community-grammar status (FR-PL-07
;    flags it second-highest-risk of the set) shows through, and it was found by
;    dumping trees rather than by reading the query back:
;      "/users/{id}"      → (string_literal (string_content))
;      "${base}/users"    → (string_literal (interpolation (identifier)) (string_content))
;      "$base/users"      → (string_literal (string_content) (string_content))   ← "$" | "base/users"
;      "/users/$id/roles" → (string_literal (string_content) (string_content) (string_content))
;    The braced form gets an `interpolation` node, which `static_string_literal`
;    already refuses. The BARE form does not: the grammar merely splits the
;    fragment at the `$`, and core concatenates the pieces back into what looks
;    like a static literal. Left alone, `uri("/users/$id/roles")` would therefore
;    bind the template `/users/$id/roles` — a runtime-composed path captured as
;    though it were static, which is exactly what NFR-RA-05 forbids. Every
;    pattern below is guarded by `(#not-match? @invoke.http.arg "[$]")`, and the
;    fluent form carries a companion pattern that re-captures the ARGUMENT node
;    (whose kind is not string-shaped) so the site is still seen and still
;    reported base-url-runtime rather than vanishing.
;
;    The guard is a predicate on the argument's OWN literal syntax — the same
;    thing `static_string_literal` inspects — not a heuristic about what the
;    surrounding code means. It is not the receiver-name kind of text predicate
;    the registration rules below deliberately avoid.
;
; 4. A TRAILING LAMBDA re-roots the tree. `f(x) { … }` does not add a child to
;    `f`'s call — it WRAPS it:
;      (call_expression (call_expression f (value_arguments x)) (annotated_lambda …))
;    so the inner call is byte-identical to a plain `f(x)`. The whole of the next
;    section rests on this one.
;
; ── Why a route registration is not a client call (the S-345 re-check) ──────
;
; A provider's own route registration is syntactically a client call. S-345's
; first Go draft captured four PHANTOM outbound calls from one Gin provider's
; own registrations, each of which would have bound another workspace member's
; matching route and fabricated a cross-service edge — the one thing this arm
; must never do (NFR-RA-05). Kotlin's mirror of that trap was hunted for
; deliberately, and it exists in four distinct shapes:
;
;   * RECEIVER-LESS DSLs — Ktor's `routing { get("/users") { … } }`, Spring
;     WebFlux's Kotlin router `router { GET("/users") { … } }`, Spark's
;     `get("/users") { … }`. All parse as `(call_expression (identifier) …)`
;     with NO navigation_expression. Every pattern below requires a receiver, so
;     all three are refused structurally.
;
;   * CLASS-QUALIFIED — `RouterFunctions.route(GET("/users"), h)`,
;     `RequestPredicates.GET("/users")`. Refused by the receiver rule: a
;     receiver must start lower-case, i.e. be a value, never a type.
;
;   * RECEIVER-BEARING, with the handler passed POSITIONALLY —
;     `app.get("/users", handler)` (Javalin), `router.get("/x", h)`. Refused by
;     the SOLE-ARGUMENT rule: pattern 4's `value_arguments` is anchored on both
;     sides, so a registration's trailing handler argument excludes it. This is
;     Go's argument-arity discriminator, and it is a structural one — the only
;     text available is the receiver name (`client` vs `router`), which is
;     exactly the fragile heuristic CR-110 is cleaning up after.
;
;   * RECEIVER-BEARING, with the handler passed as a TRAILING LAMBDA —
;     `app.get("/users") { ctx -> … }` (Javalin), `mockMvc.get("/users/1") { … }`
;     (Spring's MockMvc Kotlin DSL, which calls this service's OWN route). By
;     grammar fact 4 the inner call is indistinguishable from a real one, and a
;     tree-sitter query cannot negate a PARENT. So pattern 4 is instead anchored
;     POSITIVELY on the positions a call can occupy when nothing consumes it as a
;     callee: a statement in a block, an expression body, a `return`, or a
;     property initialiser. A lambda-applied call sits one level deeper — it is
;     the callee of the wrapper — and is structurally excluded. This is S-345's
;     second discriminator (value-position vs bare expression statement) in the
;     shape Kotlin's grammar forces.
;
; The cost of the last two rules is stated, tested and deliberate; see the
; ceilings section at the foot of this file.
;
; ── Patterns ────────────────────────────────────────────────────────────────

; 1. Fluent verb-then-`uri` with a bare path literal — `RestClient`/`WebClient`,
;    the dominant real-world Spring shape and the S-341 fluent-chain decision:
;      restClient.get().uri("/users/{id}").retrieve().body(String::class.java)
;      webClient.post().uri("/orders/{id}").retrieve().awaitBody<Order>()
;
;    The anchor is the ADJACENT link pair: the verb link's member name and the
;    `.uri(…)` link's sole argument. The verb link's own receiver is `(_)`, so an
;    EXTENSION-FUNCTION receiver (`fun RestClient.byId(id: String) =
;    this.get().uri("/users/{id}")…`, whose receiver is a `this_expression`
;    rather than an identifier) is captured by the same pattern.
;
;    A chain that separates the verb from `.uri(…)` is refused WHOLE — a match
;    that does not bind both slots emits nothing.
(call_expression
  (navigation_expression
    (call_expression
      (navigation_expression
        (_)
        (identifier) @invoke.http.method
        .)
      (value_arguments))
    (identifier) @_uri
    .)
  (value_arguments
    .
    (value_argument
      (_) @invoke.http.arg
      .)
    .)
  (#eq? @_uri "uri")
  (#not-match? @invoke.http.arg "[$]"))

; 1b. The same fluent shape whose `.uri(…)` argument carries a Kotlin STRING
;     TEMPLATE (grammar fact 3). Pattern 1's guard refuses it, and this one
;     re-captures it on the ARGUMENT node rather than on the literal: a
;     `value_argument` is not string-kinded, so `static_string_literal` returns
;     None and the dispatch sets the dynamic-path marker. The site is therefore
;     still seen and still refused as base-url-runtime, instead of disappearing
;     and being reported as nothing at all.
;
;     `"$base/users"` would be refused anyway — it reads back relative — but
;     `"/users/$id/roles"` reads back absolute, and without this pair it would
;     BIND a runtime-composed path (NFR-RA-05). The two patterns are mutually
;     exclusive by construction: one requires a `$` in the argument's source
;     text, the other forbids it.
(call_expression
  (navigation_expression
    (call_expression
      (navigation_expression
        (_)
        (identifier) @invoke.http.method
        .)
      (value_arguments))
    (identifier) @_uri_interpolated
    .)
  (value_arguments
    .
    (value_argument) @invoke.http.arg
    .)
  (#eq? @_uri_interpolated "uri")
  (#match? @invoke.http.arg "[$]"))

; 2. Fluent verb-then-`uri` where the path is wrapped in `URI.create(…)` —
;    `java.net.http.HttpClient` with the verb set before the URI:
;      HttpRequest.newBuilder().POST(body).uri(URI.create("/items")).build()
;    The literal is captured from INSIDE the constructor call, so the site is a
;    static path rather than a refused `URI.create(…)` expression.
(call_expression
  (navigation_expression
    (call_expression
      (navigation_expression
        (_)
        (identifier) @invoke.http.method
        .)
      (value_arguments))
    (identifier) @_uri_ctor
    .)
  (value_arguments
    .
    (value_argument
      (call_expression
        (navigation_expression
          (identifier) @_uri_type
          (identifier) @_create
          .)
        (value_arguments
          .
          (value_argument
            (string_literal) @invoke.http.arg
            .)
          .))
      .)
    .)
  (#eq? @_uri_ctor "uri")
  (#eq? @_uri_type "URI")
  (#eq? @_create "create")
  (#not-match? @invoke.http.arg "[$]"))

; 3. `uri`-then-verb with the path wrapped in `URI.create(…)` — the canonical
;    `java.net.http.HttpClient` builder order:
;      HttpRequest.newBuilder().uri(URI.create("/items/{id}")).GET().build()
;    Here the verb is the OUTER link and the path the inner one, so the two slots
;    swap roles relative to pattern 2.
(call_expression
  (navigation_expression
    (call_expression
      (navigation_expression
        (_)
        (identifier) @_uri_outer
        .)
      (value_arguments
        .
        (value_argument
          (call_expression
            (navigation_expression
              (identifier) @_uri_type_outer
              (identifier) @_create_outer
              .)
            (value_arguments
              .
              (value_argument
                (string_literal) @invoke.http.arg
                .)
              .))
          .)
        .))
    (identifier) @invoke.http.method
    .)
  (#eq? @_uri_outer "uri")
  (#eq? @_uri_type_outer "URI")
  (#eq? @_create_outer "create")
  (#not-match? @invoke.http.arg "[$]"))

; 4. The plain receiver-method idiom — `RestTemplate`'s bare-verb methods:
;      restTemplate.delete("/carts/{id}")
;      restTemplate.delete(url = "/carts/{id}")     ; named argument (fact 2)
;      this.restTemplate.delete("/carts/{id}")
;
;    Three constraints, all structural, all justified in the registration
;    section above:
;      (a) a RECEIVER is required, and a bare-identifier receiver must start
;          lower-case — a value, never a type;
;      (b) the path must be the SOLE argument;
;      (c) the call must occupy a value/statement POSITION rather than be the
;          callee of a trailing-lambda application.
;
;    (c) is why this appears as four near-identical branches: the position is the
;    parent, and a query can only state a parent positively.
[
  ; `restTemplate.delete("/carts/{id}")` as a statement in a block.
  (block
    (call_expression
      (navigation_expression
        [
          (identifier) @_recv
          (navigation_expression (this_expression) (identifier) @_recv)
        ]
        (identifier) @invoke.http.method
        .)
      (value_arguments
        .
        (value_argument
          (_) @invoke.http.arg
          .)
        .)
      .))
  ; `fun drop(id: String) = restTemplate.delete("/carts/$id")` — expression body.
  (function_body
    (call_expression
      (navigation_expression
        [
          (identifier) @_recv
          (navigation_expression (this_expression) (identifier) @_recv)
        ]
        (identifier) @invoke.http.method
        .)
      (value_arguments
        .
        (value_argument
          (_) @invoke.http.arg
          .)
        .)
      .))
  ; `return restTemplate.delete("/carts/{id}")`.
  (return_expression
    (call_expression
      (navigation_expression
        [
          (identifier) @_recv
          (navigation_expression (this_expression) (identifier) @_recv)
        ]
        (identifier) @invoke.http.method
        .)
      (value_arguments
        .
        (value_argument
          (_) @invoke.http.arg
          .)
        .)
      .))
  ; `val body = restTemplate.getForObject("/carts/{id}", String::class.java)` —
  ; a property initialiser. (That particular call is a stated ceiling for a
  ; different reason; the position is legitimate.)
  (property_declaration
    (call_expression
      (navigation_expression
        [
          (identifier) @_recv
          (navigation_expression (this_expression) (identifier) @_recv)
        ]
        (identifier) @invoke.http.method
        .)
      (value_arguments
        .
        (value_argument
          (_) @invoke.http.arg
          .)
        .)
      .))
]
(#match? @_recv "^[a-z_]")
(#not-match? @invoke.http.arg "[$]")

; ── Stated coverage ceilings (ADR-54: recorded, never worked around) ────────
;
; NOT captured. Each is asserted as ZERO in
; logos-core/tests/kotlin_http_client_call.rs §5 — the tests are the
; enforcement, this list is only the index, so a ceiling cannot quietly become a
; lie:
;
;   * An IMPLICIT extension-function receiver — `fun RestClient.byId(id: String)
;     = get().uri("/users/{id}")…`, where `get()` parses as
;     `(call_expression (identifier) (value_arguments))` with no receiver at all.
;     Refused by the receiver rule, which is the same rule that refuses Ktor's
;     and WebFlux's receiver-less route DSLs; the two are indistinguishable, and
;     refusing both is the NFR-RA-05 direction. Spelling `this.` captures.
;
;   * A TRAILING-LAMBDA call — `restClient.get().uri { it.path("/users").build() }`
;     and any `receiver.verb("/p") { … }`. The first has no `value_arguments` at
;     all; the second is excluded by pattern 4's position rule, because that is
;     what a Javalin/MockMvc-DSL route registration looks like. Both refuse.
;
;   * A MULTI-ARGUMENT receiver-method call — `restTemplate.put("/carts/{id}",
;     body)`, `restTemplate.delete(url = "/p", x = y)`. Java captures the first;
;     Kotlin does not, because Kotlin has route-registration APIs of exactly that
;     shape (`app.get("/users", handler)`) and Java does not. A deliberate,
;     tested divergence in the safe direction.
;
;   * A receiver-method call in any OTHER position than pattern 4's four —
;     nested as an argument, or extended by a further `.also { … }`. Under-capture
;     is the price of the position rule.
;
;   * `RestTemplate`'s verb-suffixed methods (`getForObject`, `postForEntity`,
;     `exchange`, …) and OpenFeign `@FeignClient` interfaces — identical to
;     Java's ceilings and for the identical reason: the arm's
;     `@invoke.http.method` slot needs a node whose TEXT is literally an HTTP
;     verb, and these encode it in a method name or an annotation name. Lifting
;     them needs a descriptor-level method-alias table (CR-108 CRA-05).
;
;   * A runtime-composed path on the PLAIN RECEIVER-METHOD idiom
;     (`restTemplate.delete("/carts/$id")`) is refused, but silently: pattern 4
;     carries the `$` guard without the companion pattern 1b gives the fluent
;     form, so the site is never seen and no base-url-runtime reason is reported
;     for it. A NON-interpolated dynamic path there (`restTemplate.delete(url)`)
;     still reports normally. Narrow, and in the safe direction.
;
;   * A separated JDK builder chain (`…uri(URI.create("/p")).header("a","b").GET()`)
;     and `HttpRequest.newBuilder(URI.create("/p"))`, which has no verb link —
;     both refuse whole rather than emitting a verb-less or path-less half.
;
;   * OkHttp, Ktor's client and Retrofit — outside FR-WS-08's normative Kotlin
;     row.
;
; One ceiling is an OVER-capture, not an under-capture: the ledger gate is
; file-grained, so a route-shaped collection call inside a genuine client file
; (`perms.get("/admin/users")`) still captures. Inherited from the Rust arm and
; from Java, likewise pinned by a test; no query can separate it from
; `client.get(…)` without receiver typing.
;
; Like every capability query this file is droppable-on-disk: a copy at
; `.logos/plugins/kotlin/queries/invocations.scm` shadows it without a rebuild
; (FR-PL-04, FR-PL-05).
