; C# HTTP client-call capture (S-346, capability = "invocations", FR-WS-08,
; CR-108). `HttpClient` — the one client library FR-WS-08's normative C# row
; names — plus its `System.Net.Http.Json` extension methods.
;
; Capture contract (extract::collect_invocation_sites): exactly two names, and
; the code — not the query — makes every judgment.
;
;   @invoke.http.method — a node whose TEXT is the verb. Resolved through this
;                         plugin's `[invocation_methods]` table, then kept only
;                         when `is_http_method` recognises the RESULT.
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
; anchors are evaluated only in a file referencing `System::Net::Http`, so a
; same-shaped `cache.Get("k")` elsewhere is never scanned at all (FR-WS-08
; shared negative-case fixture contract, case 1).
;
; ── The normalizer table this story owns ────────────────────────────────────
;
; C# is the first language whose verbs are spelled in neither of the two forms
; the arm already understood. `extract::is_http_method` compares against BARE
; verbs (`get`, `post`, …), and C# offers:
;
;   * an `Async` SUFFIX on the method name — `GetAsync`, and `getasync` is not
;     `get`; and
;   * a NAMED CONSTANT in a constructor argument — `HttpMethod.Get`, whose only
;     available texts are `HttpMethod.Get` and `Get`.
;
; Both are resolved by ONE descriptor table, `[invocation_methods]` in
; `plugin.toml` — the consumer-side twin of `[framework_methods]`, read by
; `extract::normalize_invocation_method`. S-345 measured the second shape as a
; stated ceiling in Go (`http.NewRequest(http.MethodGet, …)`) and deferred it to
; exactly this table; see "Go's ceiling" below for why it is still a ceiling
; there.
;
; The table also FILTERS: a captured text with no row is dropped. That is what
; makes the receiver-method anchor below safe here without an arity
; discriminator — see the next section.
;
; ── Why an ASP.NET Core registration is not a call ──────────────────────────
;
; A provider's own route registration is syntactically a client call. ASP.NET
; Core minimal APIs spell `app.MapGet("/users", Handler)`, which parses as
;
;   invocation_expression
;     member_access_expression [identifier . identifier]
;     argument_list  (argument (string_literal)) (argument (identifier))
;
; — byte-for-byte the tree of `client.PostAsync("/users", content)`. An ASP.NET
; Core service that also does outbound calls references `System.Net.Http` too, so
; the ledger gate does not separate them, and capturing a registration would turn
; a provider's own surface into phantom outbound calls binding other members'
; routes — a fabricated cross-service edge (NFR-RA-05). The Go arm needed two
; STRUCTURAL discriminators for this, because Gin's `r.GET(...)` and the stdlib's
; `http.Get(...)` share a verb NAME and only their argument shapes differ.
;
; C# does not have that collision: the registration vocabulary (`MapGet`,
; `MapPost`, `MapPut`, `MapDelete`, `MapMethods`, `Map`) and the client
; vocabulary (`GetAsync`, `PostAsync`, `GetFromJsonAsync`, …) are DISJOINT
; identifier sets. So the discriminator is the `[invocation_methods]` name pin,
; which is a genuine structural fact about the two APIs — not the fragile
; receiver-name heuristic (`c`/`client` vs `app`/`router`) CR-110 is cleaning up
; after. `MapGet` has no row, so it never reaches `is_http_method`.
;
; The same pin closes the shape Go and Java can only record as an ADR-54 ceiling:
; a bare `_cache.Get("/health")` inside a genuine client file passes the bare-verb
; filter in those languages, but `Get` has no row here, so it is refused. The
; residual C# ceiling is narrower and named: a non-HTTP receiver exposing a method
; that IS a row (`IDistributedCache.GetAsync(key)`) inside a `System.Net.Http`
; file, and only when its key is a `/`-prefixed literal that normalizes.
;
; ── Go's ceiling is not lifted by this table ────────────────────────────────
;
; `[invocation_methods]` is PER-PLUGIN data, and `plugins/go/plugin.toml`
; declares none — so Go's pass-through behaviour is byte-identical to before, and
; `go_invocations.rs::a_named_constant_verb_is_a_stated_ceiling_not_a_capture`
; stays green. Go's query is independently unable to reach the shape in any case:
; its constructor pattern binds `@invoke.http.method` to an
; `interpreted_string_literal_content`, and `http.MethodGet` is a
; `selector_expression`. Lifting Go's ceiling is a Go-side decision — a table plus
; a query pattern — not a side effect of this story.
;
; Deliberately NOT captured: `client.Send(request)` / a `SendAsync(req)` whose
; `HttpRequestMessage` was built elsewhere — the constructor pattern below
; captures the CONSTRUCTOR wherever it sits, so the split spelling is covered by
; that pattern rather than by the send; but a request built by a helper function
; needs dataflow the extractor does not have. `new HttpMethod("GET")` — see the
; `[invocation_methods]` header for why bare-verb rows are refused.
;
; Droppable on disk at `.logos/plugins/c-sharp/queries/invocations.scm`
; (FR-PL-04, FR-PL-05).

; ── Verb-as-method-name, path is the first argument ─────────────────────────
; `client.GetAsync("/users")`, `client.PostAsync("/users", content)`,
; `await _client.DeleteAsync("/users/{id}")` — one pattern, because the verb is
; the member name and the path is always argument one. `await` wraps the
; invocation, so anchoring on `invocation_expression` reaches both spellings.
; A runtime-composed path (`$"{base}/users"`, a bare variable) still yields a
; SITE, so the arm classifies it base-url-runtime rather than never seeing it.
(invocation_expression
  function: (member_access_expression
    name: (identifier) @invoke.http.method)
  arguments: (argument_list
    .
    (argument (_) @invoke.http.arg)))

; ── The same, with an explicit type argument ────────────────────────────────
; `client.GetFromJsonAsync<User>("/users")` — the `System.Net.Http.Json`
; extensions are generic, so the member name is a `generic_name` wrapping the
; identifier rather than a bare identifier. Same two slots.
(invocation_expression
  function: (member_access_expression
    name: (generic_name
      (identifier) @invoke.http.method))
  arguments: (argument_list
    .
    (argument (_) @invoke.http.arg)))

; ── Verb-as-constructor-argument ────────────────────────────────────────────
; `client.SendAsync(new HttpRequestMessage(HttpMethod.Get, "/users"))` and its
; split spelling
;
;   var req = new HttpRequestMessage(HttpMethod.Get, "/users");
;   await client.SendAsync(req);
;
; are one pattern: the anchor is the CONSTRUCTOR, not the send, so both are
; reached and `SendAsync` itself needs no row (it names no verb). The verb node
; is the whole `member_access_expression`, whose text `HttpMethod.Get` is the
; `[invocation_methods]` key.
;
; The type is pinned BY NAME to `HttpRequestMessage`. Nothing structural
; distinguishes an outbound request from any other two-argument construction, and
; S-345 recorded that a value-position test does not work (a bound result is
; idiomatic on both sides). An aliased or fully-qualified spelling
; (`new System.Net.Http.HttpRequestMessage(…)`) falls outside the predicate and is
; honestly uncaptured, the safe direction.
(object_creation_expression
  type: (identifier) @_type
  arguments: (argument_list
    .
    (argument (member_access_expression) @invoke.http.method)
    .
    (argument (_) @invoke.http.arg))
  (#eq? @_type "HttpRequestMessage"))
