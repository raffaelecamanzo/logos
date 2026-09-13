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
;                         string literal, or a `@ConfigurationProperties`
;                         accessor naming a committed key (S-397/S-398); a bare
;                         variable or a concatenation is refused as
;                         `base-url-runtime` (never approximately matched,
;                         NFR-RA-05). A `UriBuilder` LAMBDA is bound to the
;                         operand INSIDE it by pattern 5, and stays refused
;                         whole when the lambda composes more than that pattern
;                         admits (S-399).
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
;    The vocabulary is FR-WS-08's normative Java row — `RestClient`,
;    `RestTemplate`, `WebClient`, `java.net.http.HttpClient` — plus
;    `RestOperations`, the interface `RestTemplate` implements and is
;    conventionally injected as (that is what makes the collaborator mockable,
;    so `restOperations.delete("/p")` is an ordinary genuine call). A receiver
;    is admitted when its name:
;
;      * STARTS with a type-derived lower-camel token — `restClient`,
;        `restTemplate`, `restOperations`, `webClient`, `httpClient` — after an
;        optional `_`/`$` field prefix, and optionally followed by a
;        camel/digit-boundary suffix (`restTemplateV2`, `webClient2`,
;        `_restTemplate`). A field spelled `restTemplate…` IS a RestTemplate.
;        The boundary is what separates it from a substring test:
;        `restTemplatecache` is refused.
;      * ENDS with the upper-camel form of one: `usersRestTemplate`,
;        `paymentsWebClient`, `mailboxApiWebClient`.
;      * Is exactly `client`. The bare generic word is admitted only WHOLE —
;        the place this rule is strictest relative to TypeScript's
;        `[Aa]xios[A-Za-z0-9_$]*`. `axios` is a package name, so anything
;        spelled `axios…` is an axios instance; `client` is an ordinary English
;        word, and `clientRegistry` / `clientCache` are not HTTP clients.
;
;    A bare `Client` SUFFIX is deliberately NOT in the vocabulary, and this is
;    the sharpest line in the rule. It would admit every client protocol in
;    existence — and three of them meet all four capture conditions naturally:
;    `zkClient.delete("/config/orders")` (ZooKeeper's real API is
;    `delete(String path)` over absolute slash paths, and `delete` is an HTTP
;    verb), `redisClient.get("/config/features")` (Lettuce/Jedis expose a bare
;    `get(String key)`, and cache-aside puts the cache read in the SAME method as
;    the HTTP call, so co-occurrence with the ledger gate is idiomatic rather
;    than incidental), and `cacheClient.get(…)` — which is the same object this
;    rule already refuses when spelled `clientCache`. Admitting the suffix would
;    reopen exactly the CR-110 fabrication class on the consumer side
;    (NFR-RA-05). The cost is that a generically-named wrapper
;    (`ordersClient`) stays a stated ceiling; on the reference workspace it costs
;    nothing measured — the genuine Java receivers there end in `WebClient`
;    (`mailboxApiWebClient`, `pecServerWebClient`), which the suffix form admits.
;    `webTestClient` is deliberately NOT admitted: `WebTestClient` is Spring's
;    in-process test client, so it calls the service's OWN routes — a self-edge,
;    not cross-service coupling — and it is not in FR-WS-08's normative row. Its
;    fluent usage reaches pattern 1 regardless, which this rule does not gate.
;
;    It is a BOUNDARY rule, never a substring test — the distinction TypeScript's
;    `notaxiosCache.get("/cache/key")` was written for, where a substring test
;    fabricated a cross-service call out of a cache lookup (CR-110's class, on
;    the consumer side). Pinned on both edges, and on both receiver spellings, by
;    `the_receiver_rule_is_a_boundary_rule_over_the_normative_java_row`.
;
;    The cost is three stated ceilings, all under-capture and therefore safe
;    (NFR-RA-05: fabricating an edge is far worse than missing one) — see the
;    ceilings index below, which names each one's pin.
;
;    ONE pattern, not two: the bare-identifier receiver and the injected-field
;    receiver (`this.restTemplate.delete("/p")`, the ordinary Spring shape) are a
;    node alternation binding ONE `@_recv`, so the rule has a single copy and the
;    two spellings cannot drift apart. This is the repo's convention for exactly
;    this shape — `plugins/kotlin/queries/invocations.scm` alternates
;    `(identifier)` with a `this`-navigation the same way, and Python and
;    TypeScript alternate whole receiver nodes under one `#match?`. The second
;    branch captures the FIELD name, so the anchored rule above applies to it
;    unchanged (`this.perms` is judged on `perms`).
(method_invocation
  object: [
    (identifier) @_recv
    (field_access
      field: (identifier) @_recv)
  ]
  name: (identifier) @invoke.http.method
  arguments: (argument_list
    .
    (_) @invoke.http.arg)
  (#match? @_recv "^[_$]?(restClient|restTemplate|restOperations|webClient|httpClient)([A-Z0-9_$][A-Za-z0-9_$]*)?$|^[a-z_$][A-Za-z0-9_$]*(RestClient|RestTemplate|RestOperations|WebClient|HttpClient)$|^client$"))

; 5. Fluent verb-then-`uri` where the path is composed inside a `UriBuilder`
;    LAMBDA — the estate's second `@ConfigurationProperties` egress shape
;    (S-399, FR-WS-19):
;      webClient.get().uri(builder -> builder.path(props.getUriGetReport()))
;      webClient.get().uri(builder -> builder.path(props.getUriActiveOffer()).build(userId))
;
;    Pattern 1 already matches these calls and sees the whole lambda, which is
;    not an operand anything can read — so the site is refused as
;    `base-url-runtime`. This pattern matches the SAME call a second time and
;    binds `@invoke.http.arg` to the operand INSIDE the lambda, which the
;    generic dispatch then judges exactly as it judges a direct `.uri(<operand>)`
;    argument: a static literal fills the path slot, a `@ConfigurationProperties`
;    accessor resolves to its canonical key, and anything else refuses.
;
;    The two matches do not double-count, and the mechanism is not new here:
;    `record_refusals` cancels a refusal candidate when a RESOLVED operand of the
;    same relation lies inside its range, which is what already makes patterns
;    2-3 (`URI.create(<literal>)`) cancel pattern 1's candidate on the same call.
;    Where the inner operand does NOT resolve, both matches refuse and the two
;    candidates collapse to one row per `(relation, declaration, line)` — so an
;    unresolvable lambda records the one row it always recorded.
;
;    NUMBERED 5 rather than inserted beside patterns 1-3 it belongs with: the
;    numbering is referenced by prose in this file and by test names, and
;    append-only numbering keeps those true.
;
; ── What the lambda is allowed to compose (NFR-RA-05) ────────────────────────
;
;    `path(<one operand>)`, on the lambda's OWN parameter, optionally followed
;    by `build(…)` — and nothing else. Everything about that sentence is a
;    constraint the pattern spells, because the alternative is approximating a
;    composition:
;
;      * `@_lb_path` is `#eq?`-matched, never prefix-matched, so `pathSegment(…)`
;        — a real `UriBuilder` method taking a runtime segment — is not read as
;        `path`.
;      * The `path` argument list is anchored on BOTH ends, so a two-argument
;        `path(a, b)` binds nothing. Spring's `UriBuilder` declares no such
;        overload today, so this is the droppable-query guard (FR-PL-04) and a
;        guard against a same-named method on some other builder — not a probe
;        of a shape the estate writes.
;      * `@_lb_recv` must be the `@_lb_param` the lambda declares, so a `path(…)`
;        call on some other object that merely happens to sit in this position is
;        not read as the builder's.
;      * `build` is the only admitted terminal. It expands the template
;        variables, which the direct `.uri(template, a, b)` spelling passes as
;        trailing arguments pattern 1 also ignores — so admitting it emits the
;        SAME reference, not a wider one.
;
;    Everything else stays REFUSED WHOLE rather than binding on its resolvable
;    half. `queryParam(…)`/`queryParams(…)` are the ones that cost the most —
;    8 of the 13 `.uri(<lambda>)` sites in the reference workspace chain at least
;    one (counted 2026-09-13) — and they are refused on the same rule, not
;    exempted: a chain the arm reads only part of is a composition it has not
;    proven (NFR-RA-05).
;
;    ONE pattern, not two, following pattern 4's convention: the bare
;    `path(…)` body and the `path(…).build(…)` body are a node alternation
;    binding ONE `@invoke.http.arg`, so the shapes cannot drift apart.
;
;    Stated ceiling: `parameters:` is constrained to a bare `(identifier)`, so a
;    parenthesised or typed lambda parameter (`(builder) ->`, `(UriBuilder b) ->`)
;    is not reached. Under-capture, and no such site exists in the reference
;    workspace.
(method_invocation
  object: (method_invocation
    name: (identifier) @invoke.http.method
    arguments: (argument_list))
  name: (identifier) @_uri_lambda
  arguments: (argument_list
    .
    (lambda_expression
      parameters: (identifier) @_lb_param
      body: [
        (method_invocation
          object: (identifier) @_lb_recv
          name: (identifier) @_lb_path
          arguments: (argument_list . (_) @invoke.http.arg .))
        (method_invocation
          object: (method_invocation
            object: (identifier) @_lb_recv
            name: (identifier) @_lb_path
            arguments: (argument_list . (_) @invoke.http.arg .))
          name: (identifier) @_lb_build
          arguments: (argument_list))
      ]))
  (#eq? @_uri_lambda "uri")
  (#eq? @_lb_path "path")
  (#eq? @_lb_build "build")
  (#eq? @_lb_recv @_lb_param))

; ── Stated coverage ceilings (ADR-54: recorded, never worked around) ─────────
;
; NOT captured. Each one is asserted in
; logos-core/tests/java_http_client_call.rs — as **zero** references, or, where
; the fixture must also prove the file was scanned, as the positive control's
; reference **alone**. The tests are the enforcement and this list is only the
; index, so a ceiling cannot quietly become a lie; an entry here that names no
; pin is that lie:
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
;     field name carries no client token (`anyGateway.get("/p")`, `ordersClient`)
;     — all by pattern 4's receiver rule (S-375). Pinned by
;     `a_chained_receiver_and_a_token_less_wrapper_are_stated_ceilings` and by
;     the reject set of
;     `the_receiver_rule_is_a_boundary_rule_over_the_normative_java_row`, each as
;     the positive control alone rather than as zero.
;   * OkHttp and Apache HttpClient — outside FR-WS-08's normative Java row.
;   * A `UriBuilder` lambda that composes MORE than `path(<one operand>)` and an
;     optional `build(…)` — `queryParam(…)`, a second path segment, any other
;     terminal (S-399). Refused whole, never bound on the resolvable half
;     (NFR-RA-05). This is the most expensive ceiling in the list on the
;     reference workspace: 8 of its 13 `.uri(<lambda>)` sites chain at least one
;     `queryParam` (counted 2026-09-13). Pinned by
;     `a_uri_builder_lambda_that_chains_past_path_stays_refused_whole`.
;   * A `UriBuilder` lambda whose parameter is parenthesised or typed
;     (`(builder) ->`, `(UriBuilder b) ->`) — pattern 5 constrains `parameters:`
;     to a bare `(identifier)`. Zero such sites in the reference workspace.
;     Pinned, with the composer rule's other near misses, by
;     `the_uri_builder_composer_rule_is_probed_with_its_near_misses`.
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
; The remaining OVER-capture, narrowed but not gone (S-375, CR-120). This query
; used to carry a blanket one: the ledger gate is file-grained, so ANY
; route-shaped collection call inside a genuine client file
; (`perms.get("/admin/users")`) captured, "because no query can separate it from
; `client.get(…)` without receiver typing". That impossibility claim is retired
; — separating them needs a receiver NAME rule, not receiver typing, and pattern
; 4 now carries one, so the test that pinned the blanket over-capture is
; inverted (`a_route_shaped_collection_get_on_a_non_client_receiver_is_refused`).
;
; What survives is the FLUENT arm: patterns 1-3 and 5 constrain the `.uri` link
; — its name, and the `URI.create` receiver type or the lambda's composition —
; but place NO constraint on the receiver of the verb link, so
; `perms.get().uri("/admin/users")` in a gate-admitted file still captures.
; Pinned as the stated residual by
; `the_fluent_arm_receiver_is_an_unguarded_over_capture_ceiling`.
;
; S-399 added a SECOND over-capture residual, and a different one: pattern 5 is
; the first shape that puts the captured operand inside a BINDER, and the
; accessor hop behind it (`extract::config::accessor::DeclaredTypes::get`)
; answers from the file's declarations "at any position" — which a lambda
; parameter is not one of. A lambda whose parameter SHADOWS a
; `@ConfigurationProperties` field, and which reads an accessor off that
; parameter, binds the field's key: a key the source does not prove
; (NFR-RA-05). It is unreachable in Spring, and that is an accident of a
; third-party API rather than a guard — `[properties] accessor_prefixes` is
; `["get", "is"]` and `UriBuilder` declares no `getX()`/`isX()`, so such a
; lambda does not compile. Widening those prefixes, or giving another builder
; language this pattern, reopens it. Pinned as the capture it is by
; `a_lambda_parameter_shadowing_a_bound_field_is_a_stated_over_capture`.
;
; Both residuals are left as ceilings rather than closed, deliberately. Closing
; the SHADOWING one structurally would mean splitting pattern 5's operand
; wildcard into member-call and non-member-call alternatives, so the operand's
; receiver could be `#not-eq?`'d against the lambda parameter — four branches
; for a hazard the API already blocks. For the RECEIVER one: pattern 1's inner
; `object:` is legitimately a `method_invocation`
; (`WebClient.create(base).get().uri(…)`) and patterns 2-3's is a class name, so
; a receiver rule there would trade this over-capture for new UNDER-capture on
; the estate's dominant real shape. That trade needs a decision, not a
; mechanical edit. The shape it admits is narrow — a non-client receiver with a
; no-argument HTTP-verb-named method chained into `.uri(<literal>)` — and no
; such site exists in the reference workspace's 94 Java sites. It matters most
; to S-374, which will write one refusal row per site: an unguarded receiver
; there manufactures a coverage denominator out of ordinary code, the hazard
; CR-120 §7 names.
;
; Like every capability query this file is droppable-on-disk: a copy at
; `.logos/plugins/java/queries/invocations.scm` shadows it without a rebuild
; (FR-PL-04, FR-PL-05).
