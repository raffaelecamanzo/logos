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
;                         NFR-RA-05). A `UriBuilder` LAMBDA composes its path
;                         rather than passing one, so it binds no operand of its
;                         own: patterns 5/5a-5c declare the composer vocabulary
;                         and `extract::composer` reconciles them into the
;                         operand INSIDE the lambda, which then fills this slot
;                         (S-399, S-405). A chain carrying a link that cannot be
;                         proven unable to alter the path template stays refused
;                         whole (CR-129).
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
;      webClient.get().uri(builder -> builder.path(props.getUriActiveOffer()).build(userId))
;
;    Pattern 1 already matches these calls and sees the whole lambda, which is
;    not an operand anything can read — so the site is refused as
;    `base-url-runtime`. This pattern matches the SAME call a second time and
;    hands the lambda's body to `extract::composer`, which reconciles it against
;    patterns 5a-5c below and yields the operand INSIDE the lambda. The generic
;    dispatch then judges that operand exactly as it judges a direct
;    `.uri(<operand>)` argument: a static literal fills the path slot, a
;    `@ConfigurationProperties` accessor resolves to its canonical key, and
;    anything else refuses.
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
;    numbering is referenced by prose in this file and in
;    `logos-core/tests/java_http_client_call.rs`, and append-only numbering
;    keeps those references true rather than requiring a sweep. S-405's three
;    vocabulary patterns are 5a-5c for the same reason: they are this pattern's
;    parts, not four independent shapes, and a reader who follows a reference to
;    "pattern 5" should land on all of them.
;
; ── What the lambda is allowed to compose (NFR-RA-05, CR-129) ────────────────
;
;    `path(<one operand>)` on the lambda's OWN parameter, composed with links
;    that PROVABLY CANNOT ALTER THE PATH TEMPLATE, and optionally terminated by
;    `build(…)`.
;
;    S-399 shipped the narrower reading — `path(<one operand>)` and nothing
;    beyond `build()` — and CR-129 clarified it against the requirement it
;    implements. FR-WS-08 AC2 refuses a path "composed from a NON-RESOLVABLE
;    operand", and a query parameter does not compose the path at all, so
;    refusing the site on its account was stricter than FR-WS-08 asks. It cost
;    the estate's dominant lambda shape: 6 of the 9 `src/main` `.uri(<lambda>)`
;    sites on the reference workspace chain at least one `queryParam`-family
;    link (counted 2026-09-14, CR-129 §8.2 — a CENSUS of one estate, never a
;    floor).
;
; ── The path-neutrality contract test (CR-129 AC1) ───────────────────────────
;
;    A link is path-neutral IFF IT NAMES A URI COMPONENT THAT IS NOT THE PATH.
;    That is a TEST, not a list of admitted method names, and the difference is
;    the whole of CR-129's first criterion — a method-name whitelist is the
;    over-capture shape CR-110 established, and it would already have shipped
;    with a hole:
;
;      * The vocabulary pattern 5b spells is RFC 3986's COMPONENT set — scheme,
;        userInfo, host, port, query, fragment. That is the URI grammar, not
;        Spring's API surface. `UriBuilder` names every component mutator after
;        the component it mutates, and a link naming the query component cannot
;        reach the path component: they are disjoint parts of one URI. This is
;        FR-CG-09's template — what a provider route is matched on — and it is
;        what "path-neutral" is defined against.
;      * THE ESTATE IS WHY THIS IS A TEST. It composes with `queryParam`,
;        `queryParamIfPresent` AND `queryParams`; the last two were not
;        anticipated when the rule was sketched (CR-129 §8.3, decision 3), and a
;        whitelist would have refused them. All three name the query component
;        on arrival, as would a `queryParamIfMissing` nobody has written yet.
;      * A link naming NO component — `encode()`, `normalize()`,
;        `cloneBuilder()`, `toUriString()` — is NOT proven neutral and refuses.
;        The rule fails CLOSED: what it cannot prove, it refuses (NFR-RA-05).
;      * `uri(URI)` is why the test is the component vocabulary and NOT "the
;        name does not contain `path`". That method resets every component
;        INCLUDING the path while naming none of them, so a `[Pp]ath`-absence
;        test would have admitted it; here it names no component and refuses.
;      * The `[Pp]ath` guard in 5b is a SECOND, redundant test, kept because a
;        link naming the path component is never neutral however else it is
;        named (`queryAndPath`). It is redundant against today's vocabulary and
;        is what stays true if that vocabulary is ever widened.
;
;    Everything S-399 constrained about the path link itself is UNCHANGED, and
;    CR-129 §3.3 puts it explicitly out of scope — which operands are resolvable
;    does not move:
;
;      * `@_lb_path` is `#eq?`-matched, never prefix-matched, so `pathSegment(…)`
;        — a real `UriBuilder` method taking a runtime segment — is not read as
;        `path`. It is refused twice over: it binds no operand here, and it is
;        not path-neutral there.
;      * The `path` argument list is anchored on BOTH ends, so a two-argument
;        `path(a, b)` and a zero-argument `path()` bind nothing. Spring's
;        `UriBuilder` declares no such overload today, so this is the
;        droppable-query guard (FR-PL-04) and a guard against a same-named
;        method on some other builder — not a probe of a shape the estate writes.
;      * `@invoke.http.composer.receiver` must be the `@invoke.http.composer
;        .param` the lambda declares, so a `path(…)` call on some other
;        object that merely happens to sit in this position is not read as the
;        builder's.
;      * `build` is the only admitted terminal, and only as the chain's
;        OUTERMOST link. It expands the template variables, which the direct
;        `.uri(template, a, b)` spelling passes as trailing arguments pattern 1
;        also ignores — so admitting it emits the SAME reference, not a wider
;        one. A call AFTER it operates on what it returned — a `URI`, not the
;        builder — so the builder's contract proves nothing about it, and
;        `build().normalize()` really does rewrite the path it was handed.
;      * A SECOND `path(…)` anywhere inside the chain refuses it: a template
;        composed from two operands is a composition this arm has not read, and
;        it reads one (NFR-RA-05).
;
; ── Why FOUR patterns and a byte-range reconciliation ────────────────────────
;
;    A composer chain is arbitrarily long — 4 links at the estate's shortest
;    chained site and 9 at its longest — and a tree-sitter pattern's nesting is
;    FIXED. No single pattern can hold both the call's verb link and an operand
;    buried under an unknown number of links, so the four below each declare one
;    part of the vocabulary and `extract::composer` reconciles their matches by
;    byte range: the chain's links are exactly the nodes sharing its start
;    offset, and every one of them between the path link and the root must have
;    been matched as neutral (or, for the root alone, as the terminal). A link
;    NO pattern classified is a link nothing proved harmless, so the chain
;    refuses — the reconciliation fails closed, which is what keeps the rule
;    inside NFR-RA-05.
;
;    The alternative was hand-unrolling the chain to a fixed depth in the
;    pattern itself. That is a bounded rule wearing a general one's clothes: it
;    would refuse the estate's 9-link site for no reason a reader could state,
;    and each depth is a hand-written copy free to drift from its siblings —
;    which is the hazard S-399 wrote its "ONE pattern, not two" note against,
;    reached by a different road.
;
;    The bare (unterminated) composer body is DEFENSIVE and pattern 4's two
;    receiver spellings are not — worth stating so a later author does not read
;    it as an estate shape. Spring's overload is `uri(Function<UriBuilder, URI>)`
;    and `UriBuilder.path(String)` returns `UriBuilder`, so `builder ->
;    builder.path(x)` does not compile against it: all 13 `.uri(<lambda>)` sites
;    on the reference workspace carry the terminal, and that is the API, not the
;    corpus. It costs nothing here — the terminal is simply optional in the
;    reconciliation — and is pinned by the `bare` row of
;    `a_uri_builder_lambda_yields_the_reference_the_direct_form_does`.
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
      parameters: (identifier) @invoke.http.composer.param
      body: (method_invocation) @invoke.http.composer))
  (#eq? @_uri_lambda "uri"))

; 5a. The composer's PATH LINK — the one link whose operand reaches the path
;     template, and the only node in the chain this arm reads an operand from.
;     Matched wherever it occurs; the reconciliation is what decides whether it
;     is INSIDE a composer, and whether it is that composer's innermost link.
((method_invocation
   object: (identifier) @invoke.http.composer.receiver
   name: (identifier) @_lb_path
   arguments: (argument_list . (_) @invoke.http.composer.operand .))
 @invoke.http.composer.path
 (#eq? @_lb_path "path"))

; 5b. A PATH-NEUTRAL link — the contract test, stated above. `object:` is
;     constrained to a `method_invocation` because a link of a fluent chain is
;     always called on the link below it, which narrows this from "every
;     component-shaped call in the file" to "every chained one".
;
;     The two alternations are one rule read at a camelCase word boundary: a
;     component names itself either at the start of the method name (`queryParam`,
;     `port`) or capitalised after a prefix (`replaceQueryParam`). Spelling the
;     boundary is what keeps `support()` from reading as the `port` component —
;     the near miss this predicate was probed with.
((method_invocation
   object: (method_invocation)
   name: (identifier) @_lb_neutral)
 @invoke.http.composer.neutral
 (#match? @_lb_neutral "^(scheme|userInfo|host|port|query|fragment)([A-Z][A-Za-z0-9_$]*)?$|^[a-z][A-Za-z0-9_$]*(Scheme|UserInfo|Host|Port|Query|Fragment)([A-Z][A-Za-z0-9_$]*)?$")
 (#not-match? @_lb_neutral "[Pp]ath"))

; 5c. The composer's TERMINAL. Admitted by the reconciliation only as the
;     chain's outermost link, for the reason given above.
((method_invocation
   object: (method_invocation)
   name: (identifier) @_lb_build
   arguments: (argument_list))
 @invoke.http.composer.terminal
 (#eq? @_lb_build "build"))

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
;   * A `UriBuilder` lambda carrying a link the path-neutrality contract test
;     cannot prove unable to alter the path template — a second `path(…)`, a
;     `pathSegment(…)`, a call after the terminal, a terminal that is not
;     `build`, or any link naming no URI component at all (S-399, narrowed to
;     this by S-405/CR-129). Refused whole, never bound on the resolvable half
;     (NFR-RA-05). This WAS the most expensive ceiling in the list on the
;     reference workspace — 6 of its 9 `src/main` sites — and S-405 admitted
;     that population; what remains here is the residue, which the estate does
;     not write at all. Pinned by
;     `a_uri_builder_composer_link_that_reaches_the_path_stays_refused_whole`
;     and `the_uri_builder_composer_rule_is_probed_with_its_near_misses`.
;   * A `UriBuilder` lambda whose chain does not OPEN with its `path(…)` link —
;     `builder -> builder.queryParam(…).path(…).build()`. Every link in that
;     chain is provably path-neutral, so the contract test admits them all; what
;     refuses it is the separate requirement that the `path(…)` link be the
;     chain's INNERMOST one (pattern 5a's `object: (identifier)`, and the
;     start-offset test in `extract::composer`). Under-capture, and zero such
;     sites on the reference workspace — all 9 of its `src/main` sites are
;     path-first. Stated as its own entry because the contract-test wording of
;     the entry above does NOT cover it: a reader of that entry alone would
;     conclude this shape is admitted. Pinned by
;     `the_uri_builder_composer_rule_is_probed_with_its_near_misses`.
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
; the SHADOWING one structurally would mean splitting pattern 5a's operand
; wildcard into member-call and non-member-call alternatives, so the operand's
; receiver could be `#not-eq?`'d against the lambda parameter — two branches
; for a hazard the API already blocks. (It was four while S-399's pattern 5
; carried two body branches; S-405 moved the operand to 5a, which has one body.) For the RECEIVER one: pattern 1's inner
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
