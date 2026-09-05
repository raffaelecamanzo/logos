; PHP HTTP client-call capture (S-348, capability = "invocations", FR-WS-08,
; CR-108). Guzzle — the one client library FR-WS-08's normative PHP row names.
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
; anchors are evaluated only in a file referencing `GuzzleHttp` (this plugin's
; own `http_client_detectors` row), so a same-shaped `cache->get("k")` elsewhere
; is never scanned at all (FR-WS-08 shared negative-case fixture contract,
; case 1).
;
; ── Why a route registration is not a call ──────────────────────────────────
;
; PHP's grammar already separates a STATIC call (`Foo::bar()`, a
; `scoped_call_expression`) from an INSTANCE call (`$foo->bar()`, a
; `member_call_expression`) at the node-kind level. Laravel's route facade is
; always `Route::get('/x', $handler)` — a `scoped_call_expression` — so
; anchoring every pattern below on `member_call_expression` excludes it
; structurally, with no name or arity check standing behind that exclusion.
;
; Slim is the dangerous case, because its router is an INSTANCE:
; `$app->get('/x', $handler)` is a `member_call_expression` on a bare `$app`
; variable — the exact shape of `$client->get('/p')`. Capturing it would read a
; provider's own route table as an outbound call and fabricate a cross-service
; edge (NFR-RA-05). Two structural discriminators keep them apart, neither a
; text predicate on the receiver's name:
;
;   * ARITY — Guzzle's verb methods (`get`/`head`/`delete`/…) take the path
;     ALONE in their dominant real-world form; a registration ALWAYS passes a
;     second argument (the handler: a Closure, a bound variable, or a
;     `[Controller::class, 'method']` callable array). Requiring EXACTLY ONE
;     argument (the `.` anchors both before and after the sole `argument` node)
;     excludes every `$app->get('/x', $handler)` / `Route::get('/x', $handler)`
;     shape by construction. The cost is a stated ceiling: Guzzle's own
;     optional `array $options` second argument (`$client->get('/p', ['query'
;     => [...]])`) is not captured here — see "Stated coverage ceilings" below.
;
;   * RECEIVER SHAPE — the object must be a bare variable (`$client`) or a
;     property access on one (`$this->httpClient`), never a `scoped_call_
;     expression` (a fluent `Route::middleware([...])->get(...)` chain) or any
;     other expression. This is the same structural posture Java's receiver-
;     method idiom takes (`method_invocation object: (identifier)`), expressed
;     here as a node-KIND constraint rather than a naming-convention regex,
;     because PHP's grammar already draws the static/instance line for free.
;
; ── The constructor-argument anchor, PHP's `request()` (consumed from Go/
;    S-345's decision) ─────────────────────────────────────────────────────
;
; `$client->request('GET', '/p')` takes its verb from the FIRST ARGUMENT, not
; the method name — the same shape Go's `http.NewRequest("GET", …)` settled in
; Iteration 2. `@invoke.http.method` binds the literal's `string_content` child
; (bare `GET`, not the quoted node), exactly as `static_string_literal` expects
; when handed the sibling `@invoke.http.arg`'s whole literal node.
;
; No PHP web framework in the ratified v1 set (Laravel, and Slim as this
; story's registration trap) names an instance method `request`/`requestAsync`,
; so this anchor needs no arity ceiling: a trailing third `array $options`
; argument (`$client->request('GET', '/p', ['json' => $body])`) is harmless —
; the pattern anchors only the first two arguments, not the whole list.
;
; ── The Async suffix: a stated ceiling, not this story's normalizer to build ─
;
; `getAsync`/`postAsync`/… embed their verb IN THE METHOD NAME
; (`$client->getAsync('/p')` returns a Promise). The single-argument pattern
; below still MATCHES this call structurally — `is_http_method` is what
; refuses it, because "getAsync" is not one of the bare verbs it recognises
; (extract::DECLARED_METHOD_PREFIX's own rustdoc: a verb that is merely
; non-canonical TEXT "wants a text normalizer, not a per-pattern constant").
; When this was written that normalizer was C#'s unlanded scope, and building it
; here would have duplicated S-346. It has since LANDED, in the same sprint:
; `[invocation_methods]` in `plugin.toml`, read by
; `extract::normalize_invocation_method`, per-plugin descriptor data costing no
; logos-core change. `getAsync` IS the captured method-name text, so a row would
; resolve it and this ceiling is liftable by descriptor data alone.
;
; It stays a ceiling on a TRADE rather than on a missing mechanism, recorded so a
; later author decides it instead of rediscovering it: the table also FILTERS — a
; captured text with no row is dropped — so declaring `getAsync` obliges an
; identity row for every bare verb this pattern captures today (`get`, `post`,
; `put`, `delete`, `head`, `patch`, `options`), turning a free pass-through into
; a descriptor that must be kept complete. Pinned as a negative test, not
; silently dropped.
;
; `requestAsync('GET', '/p')` has NO such problem and IS captured: its verb
; comes from the first ARGUMENT (already bare `GET`), never from the method
; name, so the constructor-argument pattern above admits it for free —
; `requestAsync` simply joins `request` in that pattern's `#any-of?` set.
;
; Droppable on disk at `.logos/plugins/php/queries/invocations.scm` (FR-PL-04,
; FR-PL-05).

; ── Verb-as-method-name, bare `$var` or property-access receiver ────────────
; `$client->get('/p')`, `$client->head('/p')`, `$client->delete('/p')`, …, and
; `$this->httpClient->get('/p')` — the ordinary DI-injected-client shape. One
; pattern: the RECEIVER SHAPE discriminator above is a single rule ("a bare
; variable or a property access on one"), so the two node kinds share every
; other field and arity anchor.
(member_call_expression
  object: [(variable_name) (member_access_expression)]
  name: (name) @invoke.http.method
  arguments: (arguments
    .
    (argument (_) @invoke.http.arg)
    .))

; ── Verb-as-first-argument: `request()` / `requestAsync()` ──────────────────
; `$client->request('GET', '/p')`, `$client->requestAsync('GET', '/p')`. The
; verb is read from the literal's CONTENT child (bare `GET`), never the quoted
; node, so `is_http_method` sees an unquoted verb; the path capture takes the
; WHOLE literal, which `static_string_literal` itself unquotes.
;
; The verb literal is matched as EITHER of PHP's two quoting grammars —
; `string` (single-quoted, `'GET'`) or `encapsed_string` (double-quoted,
; `"GET"`) — because tree-sitter-php parses a double-quoted literal as
; `encapsed_string` even with no interpolation at all; matching only `string`
; silently drops every double-quoted verb. The `encapsed_string` alternative
; anchors its `string_content` child as the node's ONLY child (`.` before and
; after), so an interpolated verb (`"{$method}"`) still fails to match — the
; anchor is there for the exact reason `static_string_literal` requires every
; child to be static, not merely one of them.
(member_call_expression
  object: [(variable_name) (member_access_expression)]
  name: (name) @_fn
  arguments: (arguments
    .
    (argument [(string (string_content) @invoke.http.method)
               (encapsed_string . (string_content) @invoke.http.method .)])
    .
    (argument (_) @invoke.http.arg))
  (#any-of? @_fn "request" "requestAsync"))

; ── Stated coverage ceilings (ADR-54: recorded, never worked around) ────────
;
; NOT captured. Each is asserted as **zero** in
; logos-core/tests/php_http_client_call.rs — the tests are the enforcement,
; this list is only the index:
;
;   * The six Async-suffixed verb-as-method-name forms (`getAsync`,
;     `postAsync`, `putAsync`, `deleteAsync`, `patchAsync`, `headAsync`,
;     `optionsAsync`) — the method-name-embedded verb needs a text normalizer.
;     S-346's `[invocation_methods]` table now provides one and would lift this
;     by descriptor data alone; the header above states the filter-half trade
;     that keeps the rows undeclared.
;   * Guzzle's optional `array $options` second argument on a verb-as-method-
;     name call (`$client->get('/p', ['query' => [...]])`) — the exactly-one-
;     argument arity gate that keeps a Slim/Laravel registration out also
;     excludes this. Widening it needs a discriminator narrow enough to admit
;     an associative options array while still excluding a positional
;     `[Controller::class, 'method']` callable array, deliberately deferred
;     rather than invented under this story's scope.
;   * `$client->send($request)` / `$client->sendAsync($request)` — the verb and
;     path live on the `Request` object that built `$request`, and joining them
;     needs dataflow the extractor does not have (the same ceiling as Go's
;     `client.Do(req)`).
