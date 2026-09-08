; Java HTTP client-call capture (S-341, CR-108, capability = "invocations",
; FR-WS-08).
;
; Captures the *anchors* of an outbound HTTP client call and leaves every
; judgment to the generic dispatch (`extract::collect_invocation_sites`) and the
; arm's normalizer (`resolve::http_client_call`), exactly as the Rust query does
; (NFR-MA-01). A tree-sitter query cannot decide whether a method is an HTTP verb
; or whether an argument is a static path literal, so those checks stay in code:
;
;   @invoke.http.method — the node whose *text* is the HTTP verb (`get`, `GET`,
;                         `post`, …). Kept only when it is one of the HTTP verbs.
;   @invoke.http.arg    — the node holding the request path. Kept as a
;                         `"METHOD /template"` reference only when it is a static
;                         string literal; a bare variable / concatenation /
;                         builder lambda is refused as `base-url-runtime`
;                         (never approximately matched, NFR-RA-05).
;
; ── The fluent-chain decision (S-341, the shape Kotlin/Ruby/PHP/C# consume) ───
;
; Spring's dominant real-world shape puts the verb and the path on *different*
; links of one chain: `restClient.get().uri("/users/{id}").retrieve()`. This is
; CAPTURED, not refused. A `method_invocation` whose `object:` is itself a
; `method_invocation` exposes both links in one match, so the verb link's name
; and the `.uri(…)` link's first argument fill the two slots without any core
; change:
;
;     (method_invocation                      ; .uri("/users/{id}")
;       object: (method_invocation            ; .get()
;                 name: (identifier)))        ; ← the verb
;
; The anchor is the *adjacent* link pair. A chain that separates the verb from
; `.uri(…)` by another builder call is a stated ceiling (see below) — refused
; whole, never partially: a match that does not bind both slots emits nothing.
;
; ── Patterns ─────────────────────────────────────────────────────────────────

; 1. Fluent verb-then-`uri` with a bare path literal — `RestClient`/`WebClient`:
;      restClient.get().uri("/users/{id}").retrieve()
;      webClient.get().uri("/orders/{id}").retrieve().bodyToMono(…)
(method_invocation
  object: (method_invocation
    name: (identifier) @invoke.http.method
    arguments: (argument_list))
  name: (identifier) @_uri
  arguments: (argument_list
    .
    (_) @invoke.http.arg)
  (#eq? @_uri "uri"))

; 2. Fluent verb-then-`uri` where the path is wrapped in `URI.create(…)` —
;    `java.net.http.HttpClient` with the verb set before the URI:
;      HttpRequest.newBuilder().GET().uri(URI.create("/things"))
;    The literal is captured from *inside* the constructor call, so the site is a
;    static path rather than a refused `URI.create(…)` expression.
(method_invocation
  object: (method_invocation
    name: (identifier) @invoke.http.method
    arguments: (argument_list))
  name: (identifier) @_uri_ctor
  arguments: (argument_list
    .
    (method_invocation
      object: (identifier) @_uri_type
      name: (identifier) @_create
      arguments: (argument_list
        .
        (string_literal) @invoke.http.arg)))
  (#eq? @_uri_ctor "uri")
  (#eq? @_uri_type "URI")
  (#eq? @_create "create"))

; 3. `uri`-then-verb with the path wrapped in `URI.create(…)` — the canonical
;    `java.net.http.HttpClient` builder order:
;      HttpRequest.newBuilder().uri(URI.create("/items/{id}")).GET().build()
;    Here the verb is the *outer* link and the path the inner one, so the two
;    slots swap roles relative to pattern 2.
(method_invocation
  object: (method_invocation
    name: (identifier) @_uri_outer
    arguments: (argument_list
      .
      (method_invocation
        object: (identifier) @_uri_type_outer
        name: (identifier) @_create_outer
        arguments: (argument_list
          .
          (string_literal) @invoke.http.arg))))
  name: (identifier) @invoke.http.method
  (#eq? @_uri_outer "uri")
  (#eq? @_uri_type_outer "URI")
  (#eq? @_create_outer "create"))

; 4. The plain receiver-method idiom — the Rust arm's single pattern, ported:
;      restTemplate.delete("/carts/{id}")
;      restTemplate.put("/carts/{id}", body)
;      this.usersRestTemplate.get("/health")
;
;    A **receiver is required**, and it must be a CLIENT receiver. Java's
;    `method_invocation` makes `object:` optional, so an unconstrained pattern
;    also matches the receiver-less and class-qualified static-import idioms that
;    dominate Java routing and test DSLs — and those are not outbound calls at
;    all. Two of them invert direction outright, recording a *provider* route
;    declaration as an *outbound* call and (via the `invocation` intake) seeding a
;    false app-wide reachability root:
;
;      RouterFunctions.route(GET("/users"), h)   ; a route DECLARATION
;      RequestPredicates.GET("/users")           ; the same, qualified
;      mockMvc.perform(get("/api/users"))        ; MockMvcRequestBuilders
;      stubFor(get("/api/users").willReturn(ok)) ; WireMock
;      rest("/api").get("/{id}")                 ; Apache Camel route DSL
;
;    None is stopped by the other guards: the file-level ledger gate is a
;    co-locator here (a client test stubs the downstream it calls; a WebFlux BFF
;    holds both a WebClient and functional routes), `is_http_method` is
;    case-insensitive so the DSL's `GET(...)` passes, and the literals are
;    absolute and normalize cleanly.
;
; ── The receiver rule (S-375, CR-120, FR-WS-08 AC5) ──────────────────────────
;
;    The guard used to be `^[a-z_$]` — "a variable or field, never a class" —
;    which refuses all five DSL shapes above but admits EVERY lower-case
;    receiver, so `perms.get("/admin/users")` and `cache.get("/health")` became
;    route-shaped consumers. Only the file-grained ledger gate stood behind it,
;    and a gate that asks about the FILE cannot answer a question about the
;    CALL: FR-WS-08 AC5 requires a same-shaped non-HTTP call to emit nothing,
;    and says nothing about which file it sits in. S-375 makes the decision on
;    the RECEIVER.
;
;    The rule needs no receiver TYPING, which is what the ceiling this replaces
;    claimed. It is a receiver-NAME boundary rule over FR-WS-08's normative Java
;    row spelled as a field name — the posture the Python and TypeScript arms
;    have shipped since S-343/S-344, and the reason those two state no
;    file-grained ceiling at all:
;
;      * STARTS with a type-derived lower-camel token — `restClient`,
;        `restTemplate`, `webClient`, `httpClient` — optionally followed by a
;        camel/digit-boundary suffix (`restTemplateV2`, `webClient2`). A field
;        spelled `restTemplate…` IS a RestTemplate.
;      * ENDS with the upper-camel form of one, or with a bare `Client`:
;        `usersRestTemplate`, `paymentsWebClient`, `ordersClient`.
;      * Is exactly `client`. The bare generic word is admitted only WHOLE —
;        the one place this rule is stricter than TypeScript's
;        `[Aa]xios[A-Za-z0-9_$]*`. `axios` is a package name, so anything
;        spelled `axios…` is an axios instance; `client` is an ordinary English
;        word, and `clientRegistry` / `clientCache` are not HTTP clients.
;
;    It is a BOUNDARY rule, never a substring test — the distinction TypeScript's
;    `notaxiosCache.get("/cache/key")` was written for, where a substring test
;    fabricated a cross-service call out of a cache lookup (CR-110's class, on
;    the consumer side). Pinned both ways by
;    `the_receiver_rule_is_a_boundary_rule_over_the_normative_java_row`.
;
;    The cost is two stated ceilings, both under-capture and therefore safe
;    (NFR-RA-05: fabricating an edge is far worse than missing one): a chained
;    receiver (`getClient().get("/p")`) is not captured, and neither is a client
;    wrapper whose field name carries no client token (`anyGateway.get("/p")`).
;    Widening the rule to close the second would readmit the first false-positive
;    class, so it stays a ceiling and is not worked around.
(method_invocation
  object: (identifier) @_recv
  name: (identifier) @invoke.http.method
  arguments: (argument_list
    .
    (_) @invoke.http.arg)
  (#match? @_recv "^(restClient|restTemplate|webClient|httpClient)([A-Z0-9_$][A-Za-z0-9_$]*)?$|^[a-z_$][A-Za-z0-9_$]*(RestClient|RestTemplate|WebClient|HttpClient|Client)$|^client$"))

; 4b. The same idiom through a field access — `this.restTemplate.delete("/p")`,
;     the ordinary Spring injected-field shape, whose `object:` is a
;     `field_access` rather than a bare identifier. The captured node is the
;     FIELD name, so the receiver rule above applies to it unchanged.
(method_invocation
  object: (field_access
    field: (identifier) @_recv_field)
  name: (identifier) @invoke.http.method
  arguments: (argument_list
    .
    (_) @invoke.http.arg)
  (#match? @_recv_field "^(restClient|restTemplate|webClient|httpClient)([A-Z0-9_$][A-Za-z0-9_$]*)?$|^[a-z_$][A-Za-z0-9_$]*(RestClient|RestTemplate|WebClient|HttpClient|Client)$|^client$"))

; ── Stated coverage ceilings (ADR-54: recorded, never worked around) ─────────
;
; NOT captured. Each one is asserted as **zero** in
; logos-core/tests/java_http_client_call.rs §3 — the tests are the enforcement,
; this list is only the index, so a ceiling cannot quietly become a lie:
;
;   * `RestTemplate`'s verb-suffixed methods (`getForObject`, `getForEntity`,
;     `postForObject`, `postForEntity`, `postForLocation`, `patchForObject`,
;     `headForHeaders`, `optionsForAllow`) and `exchange`. (`put`/`delete` ARE
;     bare verbs and ARE captured, by pattern 4.)
;   * OpenFeign `@FeignClient` interfaces.
;   * A JDK builder whose `.uri(…)` and verb links are separated, and
;     `HttpRequest.newBuilder(URI.create("/p"))`, which has no verb link.
;   * A Java text block (`"""…"""`) path literal — statically present, but
;     `static_string_literal` does not recognise `multiline_string_fragment`.
;   * A chained receiver (`getClient().get("/p")`), and a client wrapper whose
;     field name carries no client token (`anyGateway.get("/p")`) — both by
;     pattern 4's receiver rule (S-375).
;   * OkHttp and Apache HttpClient — outside FR-WS-08's normative Java row.
;
; The first two share one root cause: the arm's `@invoke.http.method` slot needs
; a node whose *text* is literally an HTTP verb, and these encode it in a method
; name (`getForObject`) or an annotation name (`@GetMapping`). Parse-tree
; evidence in the S-341 implementation notes.
;
; This paragraph used to defer the fix as "a descriptor-level method-alias table
; the arm does not have (CR-108 CRA-05)". S-346 then BUILT exactly that table —
; `[invocation_methods]` in `plugin.toml`, read through
; `extract::normalize_invocation_method` from `collect_invocation_sites` — later
; in the same sprint. The mechanism now exists and is pure per-plugin descriptor
; data, so lifting `getForObject`/`postForEntity`/… costs this language no
; logos-core change:
;
;   * `getForObject` IS the captured method-name text, so a row resolves it;
;   * `@GetMapping` is not — the verb is an annotation name on a
;     `method_declaration`, with no `method_invocation` to anchor. OpenFeign
;     stays a ceiling for the anchor, not for the vocabulary.
;
; What holds the RestTemplate half is therefore a trade, not a missing
; mechanism, and it is stated here so a later author decides it rather than
; rediscovers it: declaring any row opts this language into the table's FILTER
; half — a captured text with no row is dropped — so every bare verb this query
; captures today (`get`, `GET`, `post`, `delete`, …) would need an identity row,
; and each new spelling becomes a descriptor edit rather than a free
; pass-through. `exchange` is unreachable either way: its verb is in a SECOND
; argument no capture binds.
;
; Every ceiling above is an UNDER-capture. This query used to carry one
; over-capture too — the ledger gate is file-grained, so a route-shaped
; collection call inside a genuine client file (`perms.get("/admin/users")`)
; captured, "because no query can separate it from `client.get(…)` without
; receiver typing". S-375 (CR-120) retired that claim: separating them needs a
; receiver NAME rule, not receiver typing, and pattern 4 now carries one. The
; test that pinned the over-capture is inverted
; (`a_route_shaped_collection_get_on_a_non_client_receiver_is_refused`).
;
; Like every capability query this file is droppable-on-disk: a copy at
; `.logos/plugins/java/queries/invocations.scm` shadows it without a rebuild
; (FR-PL-04, FR-PL-05).
