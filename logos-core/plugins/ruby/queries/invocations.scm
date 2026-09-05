; Ruby HTTP client-call capture (S-347, capability = "invocations", FR-WS-08,
; CR-108). Net::HTTP and Faraday — the ratified v1 Ruby client set.
;
; Captures the *anchors* of an outbound HTTP client call and leaves every
; judgment to the generic dispatch (`extract::collect_invocation_sites`) and the
; arm's normalizer (`resolve::http_client_call`), exactly as every other
; language's query does (NFR-MA-01). A tree-sitter query cannot decide whether a
; method is an HTTP verb or whether an argument is a static path literal, so
; those checks stay in code:
;
;   @invoke.http.method — the node whose *text* is the HTTP verb (`get`, `post`,
;                         …). Kept only when `is_http_method` recognises it.
;   @invoke.http.arg    — the node holding the request path. Kept as a
;                         `"METHOD /template"` reference only when
;                         `static_string_literal` reads a static literal out of
;                         it; anything else is refused as base-url-runtime
;                         (never approximately matched, NFR-RA-05).
;
; ── Ruby's two structural traps for a naive first-argument pattern ───────────
;
; 1. OPTIONAL PARENTHESES. `conn.get "/users"` (no parens) and `conn.get("/users")`
;    (parens) are NOT two shapes to cover separately. tree-sitter-ruby parses a
;    parenthesis-free receiver call as `command_call`, which is ALIASED to the
;    same `call` node type and exposes the same `receiver` / `method` /
;    `arguments` fields — its `arguments` is `command_argument_list` aliased to
;    `argument_list` too (`grammar.js` lines ~744-754, 810). One pattern below
;    covers both; there is no second pattern to write or forget.
;
; 2. THE `Net::HTTP.get(URI(...))` IDIOM — the request path is wrapped in a
;    `URI(...)` constructor call rather than passed as a bare string literal.
;    `static_string_literal` only recognises a literal *string* node; handed the
;    `URI(...)` call node whole, it returns `None` and the site is refused as
;    base-url-runtime even though the path is, in fact, static. That refusal
;    would be silently WRONG about *why* the site is unbound — a half-capture
;    this story is required not to ship. The DECISION (mirroring Java's
;    `URI.create(…)` unwrap, S-341): see it through. A dedicated pattern below
;    reaches past the `URI(…)` wrapper and binds `@invoke.http.arg` to the
;    string literal INSIDE it, so `Net::HTTP.get(URI("/users"))` renders exactly
;    like `Net::HTTP.get("/users")` would.
;
; ── The registration-vs-client trap (S-345's mandatory re-check) ────────────
;
; A route REGISTRATION is syntactically a client call in every language this
; sprint has touched, and Ruby is no exception: Sinatra's
; `get '/users' do … end` and a Rails `routes.draw { get '/users', to: … }`
; block are the two named risks. Both are RECEIVER-LESS calls — Sinatra's DSL
; methods and Rails' routing verbs run against an implicit `self`, never an
; explicit object (`references.scm`'s own Rails route pattern captures them via
; `(call !receiver method: (identifier) …)`, the mirror image of the constraint
; here). Every pattern below requires `receiver: (_)` — a field a receiver-less
; `call` node does not have at all, so the pattern simply never matches it. This
; is a STRUCTURAL discriminator (a required field, not a text predicate), the
; same posture S-345 settled on for Go's Gin trap: no receiver, no match, full
; stop. `ruby_http_client_call.rs` pins both named forms as negative fixtures
; inside a file that otherwise passes the ledger gate, with a positive-control
; call proving the file really was scanned.
;
; ── Ledger-gated candidacy ───────────────────────────────────────────────────
;
; Candidacy is gated upstream (`capture_http_client_call_arm`) against this
; plugin's own `http_client_detectors` (`net::http`, `faraday`) — matched
; against the reference ledger, not against these patterns. Ruby's `require`
; import target canonicalises through the same generic `import_segments`/
; `split_path_text` path every other language's does (`"net/http"` →
; `net::http`, `"faraday"` → `faraday`), so a same-shaped
; `cache.get("k")`/`params.get("k")` in a file with no such `require` is never
; scanned at all (FR-WS-08 shared negative-case fixture contract, case 1).
;
; Droppable on disk at `.logos/plugins/ruby/queries/invocations.scm` (FR-PL-04,
; FR-PL-05).

; ── 1. The receiver-method idiom — parenthesised AND parenthesis-free ───────
;
;   conn.get("/users")           ; Faraday, parenthesised
;   conn.get "/users"            ; Faraday, parenthesis-free — same key, not a
;                                ; near-miss: `command_call` aliases to `call`
;   Faraday.get("/users")        ; Faraday's module-level free function
;   Net::HTTP.get("/users")      ; Net::HTTP with a bare literal (no wrapper)
;
; `receiver: (_)` accepts an `identifier` (`conn`), a `constant` (`Faraday`) and
; a `scope_resolution` (`Net::HTTP`) alike — Ruby has no Java-style ambiguity
; between "a variable" and "a class name" needing a case-based guard here, since
; the registration trap is excluded structurally (above) rather than by naming
; convention.
(call
  receiver: (_)
  method: (identifier) @invoke.http.method
  arguments: (argument_list
    .
    (_) @invoke.http.arg))

; ── 2. `Net::HTTP.get(URI(...))` — the constructor-wrapped path ────────────
;
;   Net::HTTP.get(URI("/users"))
;
; `URI(...)` is Ruby's `Kernel#URI` — a receiver-less call whose method token is
; capitalised, so its `method:` field is a `(constant)` node (`_variable`'s
; subtypes include `constant`, grammar.js line ~695), not an `(identifier)`.
; Pinned to the exact callee name `URI`: see "only URI(...) unwraps" below.
(call
  receiver: (_)
  method: (identifier) @invoke.http.method
  arguments: (argument_list
    .
    (call
      !receiver
      method: (constant) @_uri_fn
      arguments: (argument_list
        .
        (string) @invoke.http.arg)))
  (#eq? @_uri_fn "URI"))

; ── 3. A symbol-keyed hash call — `conn.get(path: "/users")` ───────────────
;
; Ruby's trailing-keyword-argument sugar parses each `key: value` pair as a
; `pair` node directly inside the `argument_list` (no wrapping `hash` node,
; node-types.json's `argument_list` children include `pair` alongside
; `_expression`), so the generic pattern above would otherwise bind the whole
; `pair` as `@invoke.http.arg` and see it refused as base-url-runtime — true
; that it isn't captured, wrong about why. Unwrapped explicitly instead, pinned
; to the `path:` key so an unrelated keyword (`headers:`, `params:`) is not
; mistaken for the request path.
(call
  receiver: (_)
  method: (identifier) @invoke.http.method
  arguments: (argument_list
    .
    (pair
      key: (hash_key_symbol) @_key
      value: (string) @invoke.http.arg))
  (#eq? @_key "path"))

; ── Stated coverage ceilings ─────────────────────────────────────────────────
;
; NOT captured, each pinned as zero in `ruby_http_client_call.rs`:
;
;   * `URI.parse(…)`, `URI.join(…)`, or any OTHER receiver-typed constructor —
;     only the bare `URI(...)` Kernel-function form is unwrapped. A
;     `Net::HTTP.get(URI.parse("/users"))` stays honestly uncaptured; widening
;     pattern 2 to `URI.parse` needs its own decision, not a silent extension.
;   * A hash-rocket path pair (`conn.get("path" => "/users")`) — pattern 3 only
;     unwraps the `key:` colon form (its `pair` shape requires a
;     `hash_key_symbol`/aliased-identifier key, grammar.js line ~1193); the
;     `=>` form's key is a full `(string)`/`_arg` node, a different shape.
;   * A block-form Faraday request (`conn.get("/users") { |req| req.params[…] =
;     … }`) captures identically to the bare call — the block sits in a
;     separate `block:`/`do_block:` field pattern 1 never inspects — but a path
;     built *inside* the block (`conn.get { |req| req.url "/users" }`, no path
;     argument at all) is a stated ceiling: there is no `arguments:` field to
;     anchor on.
;   * `Net::HTTP.start(host) { |http| http.get("/users") }` — the `Net::HTTPGenericRequest`
;     helper form (`Net::HTTP::Get.new("/users")`) is a class-constructor call,
;     not a `.get(...)` method call; its verb rides the CONSTANT name
;     (`Get`/`Post`/…), which pattern 1's `method: (identifier)` cannot bind.
;     S-346's `[invocation_methods]` table (landed later in the same sprint) does
;     NOT lift this on its own: the table normalizes a CAPTURED text, and no
;     pattern here captures the constant — the called method's text is `new`.
;     Lifting it is a table PLUS a pattern reaching the receiver's last segment,
;     a Ruby-side decision, exactly as it is for Go's `http.MethodGet`.
