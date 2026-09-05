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
      name: (identifier) @_create
      arguments: (argument_list
        .
        (string_literal) @invoke.http.arg)))
  (#eq? @_uri_ctor "uri")
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
        name: (identifier) @_create_outer
        arguments: (argument_list
          .
          (string_literal) @invoke.http.arg))))
  name: (identifier) @invoke.http.method
  (#eq? @_uri_outer "uri")
  (#eq? @_create_outer "create"))

; 4. The plain receiver-method idiom, identical to the Rust arm's single pattern:
;      restTemplate.delete("/carts/{id}")
;      restTemplate.put("/carts/{id}", body)
;      anyClientWrapper.get("/health")
;    Broad by design — the HTTP-verb check and the static-literal check narrow it
;    in code, and the file-level ledger gate (`http_client_crates`) keeps it from
;    ever firing outside a real HTTP-client file (FR-WS-08 negative case 1).
(method_invocation
  name: (identifier) @invoke.http.method
  arguments: (argument_list
    .
    (_) @invoke.http.arg))

; ── Stated coverage ceilings (ADR-54: recorded, never worked around) ─────────
;
; These Java idioms are NOT captured, because the arm's `@invoke.http.method`
; slot needs a node whose *text* is literally an HTTP verb, and they encode the
; verb somewhere a query cannot rewrite:
;
;   * `RestTemplate`'s verb-suffixed methods — `getForObject("/p", X.class)`,
;     `getForEntity`, `postForObject`, `postForEntity`, `postForLocation`,
;     `patchForObject`, `headForHeaders`, `optionsForAllow`. The captured
;     identifier reads `getForObject`, which is not an HTTP verb, so the generic
;     dispatch drops the site. (`put`/`delete` ARE bare verbs and are captured by
;     pattern 4.)
;   * `RestTemplate.exchange("/p", HttpMethod.GET, …)` — the verb is a *second*
;     argument, and the arm's vocabulary reads only the first.
;   * OpenFeign `@FeignClient` interfaces — the verb is an annotation name
;     (`@GetMapping`) on a `method_declaration` with no call-site receiver at
;     all; there is no `method_invocation` to anchor on. Parse-tree evidence in
;     the S-341 implementation notes.
;   * `HttpRequest.newBuilder().uri(…).header(…).GET()` — an intervening builder
;     call separates the two links patterns 2/3 require, and
;     `HttpRequest.newBuilder(URI.create("/p"))` carries no verb link at all.
;   * OkHttp `new Request.Builder().url("/p").get()` — outside FR-WS-08's
;     normative Java row.
;
; Lifting the first three needs a descriptor-level method-alias table (a
; `[framework_methods]`-style `getForObject → GET` / `GetMapping → GET` map read
; by `collect_invocation_sites`) — a capture-vocabulary widening, deliberately
; deferred rather than invented here (CR-108 CRA-05).
;
; Like every capability query this file is droppable-on-disk: a copy at
; `.logos/plugins/java/queries/invocations.scm` shadows it without a rebuild
; (FR-PL-04, FR-PL-05).
