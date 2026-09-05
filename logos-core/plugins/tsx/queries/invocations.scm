; TSX/JSX HTTP client-call capture (S-343, capability = "invocations",
; FR-WS-08, CR-108). Its patterns are identical to those in
; `typescript/queries/invocations.scm`: `.ts`/`.js` and `.tsx`/`.jsx` are one
; language across two tree-sitter `Language`s (ADR-09), so a rule landed in one
; and not the other leaves half the TypeScript surface uncovered.
;
; Capture vocabulary (interpreted by `extract::collect_invocation_sites`):
;
;   @invoke.http.method     — a node whose TEXT is the verb. Kept only when it is
;                             one of the HTTP verbs.
;   @invoke.http.method.get — the verb declared by the capture NAME, for a shape
;                             that spells no verb at all. An explicit
;                             `@invoke.http.method` node always outranks it.
;   @invoke.http.arg        — the request path. Kept as a `"METHOD /template"`
;                             reference only when it is a static string literal;
;                             a bare variable or an interpolated template literal
;                             is refused as base-url-runtime, never approximately
;                             matched (NFR-RA-05).
;
; The free-function anchor question this story owns (CR-108): the generic
; dispatch is **receiver-agnostic** — it reads capture names, not call shape — so
; `fetch(…)` needs no receiver and no core dispatch change to be anchored. The
; only thing it could not express was a verb-less shape, which the
; `@invoke.http.method.<verb>` capture name now states declaratively.
;
; Every pattern here is scoped to a NAMED client (`fetch`, an `axios`-named
; receiver). That is deliberate and load-bearing: the broad
; `<receiver>.<method>(<arg>)` anchor Rust ships is the exact shape CR-110 had to
; retract on the provider side, where an Angular `formGroup.get("year")` and a
; `cache.get("/cache/key")` both promoted routes. The consumer side must not
; reintroduce that false-positive class, so the query — not just the arm's
; ledger gate — refuses an unrecognised receiver.
;
; Droppable on disk at `.logos/plugins/tsx/queries/invocations.scm`
; (FR-PL-04, FR-PL-05).

; ── axios, receiver-verb form: `axios.get("/users")`, `axiosClient.post("/u", b)`
;
; Ceiling (stated, not worked around): a tree-sitter pattern is local, so it
; cannot resolve an instance back to its `axios.create()` binding. The receiver
; is therefore constrained on its conventional NAME, optionally member-qualified
; (`this.axios`) — the same posture `frameworks.scm` takes for an Express
; `app`/`router`. A renamed instance (`const api = axios.create(…);
; api.get("/users")`) stays honestly uncaptured rather than opening the receiver
; to every object in the file.
;
; The name rule is a BOUNDARY rule, never a substring test: the segment must
; either START with `axios`/`Axios` (`axios`, `axiosClient`, `axiosInstance`) or
; END with a camelCase `Axios` (`apiAxios`, `httpAxios`). A substring test would
; admit `notaxiosCache.get("/cache/key")` — a cache lookup, fabricated into a
; cross-service call — which is exactly the CR-110 false-positive class this file
; exists not to reopen (NFR-RA-05). An all-caps `AXIOS` is deliberately outside
; the set: widening the case rule buys one unidiomatic spelling and costs the
; boundary its precision.
((call_expression
   function: (member_expression
     object: [(identifier) (member_expression)] @_axios_receiver
     property: (property_identifier) @invoke.http.method)
   arguments: (arguments
     .
     (_) @invoke.http.arg))
  (#match? @_axios_receiver "(^|\\.)([Aa]xios[A-Za-z0-9_$]*|[A-Za-z0-9_$]*Axios)$"))

; ── axios, object-argument form: `axios({url: "/users", method: "get"})` and its
; `axios.request({…})` spelling. Two patterns because tree-sitter matches
; siblings in source order and an object literal's keys carry no canonical one;
; the `#eq?` guards on the key names keep the two from cross-binding.
;
; The callee carries the SAME `(^|\.)` boundary rule as the receiver-verb form
; above, so `this.axios({…})` and `this.axiosClient.request({…})` are captured
; exactly where `this.axiosClient.post(…)` is. Anchoring only at `^` here would
; make one file disagree with itself about member-qualified receivers.
;
; The verb is a string literal, so `@invoke.http.method` binds the
; `string_fragment` INSIDE it — the node whose text is the bare `get`, not the
; quoted `"get"` that would fail the verb check.
((call_expression
   function: [(identifier) (member_expression)] @_axios_callee
   arguments: (arguments
     .
     (object
       (pair
         key: (property_identifier) @_url_key
         value: (_) @invoke.http.arg)
       (pair
         key: (property_identifier) @_method_key
         value: (string (string_fragment) @invoke.http.method)))))
  (#match? @_axios_callee "(^|\\.)([Aa]xios[A-Za-z0-9_$]*|[A-Za-z0-9_$]*Axios)(\\.request)?$")
  (#eq? @_url_key "url")
  (#eq? @_method_key "method"))

((call_expression
   function: [(identifier) (member_expression)] @_axios_callee
   arguments: (arguments
     .
     (object
       (pair
         key: (property_identifier) @_method_key
         value: (string (string_fragment) @invoke.http.method))
       (pair
         key: (property_identifier) @_url_key
         value: (_) @invoke.http.arg))))
  (#match? @_axios_callee "(^|\\.)([Aa]xios[A-Za-z0-9_$]*|[A-Za-z0-9_$]*Axios)(\\.request)?$")
  (#eq? @_url_key "url")
  (#eq? @_method_key "method"))

; ── fetch, method-bearing free-function form: `fetch("/users", {method: "POST"})`
;
; The free-function anchor: no receiver participates in the pattern at all. The
; verb comes from the init object's `method` value, so a POST resolves to POST.
;
; `window.fetch` / `globalThis.fetch` / `self.fetch` are the same global spelled
; explicitly, so both fetch patterns admit them. The guard stays an exact name
; match — a bare `x.fetch(…)` on an arbitrary receiver is a repository/queue
; refresh far more often than an HTTP call, and admitting it would be the
; CR-110 class again.
((call_expression
   function: [(identifier) (member_expression)] @_fetch
   arguments: (arguments
     .
     (_) @invoke.http.arg
     .
     (object
       (pair
         key: (property_identifier) @_method_key
         value: (string (string_fragment) @invoke.http.method)))))
  (#match? @_fetch "^((window|globalThis|self)\\.)?fetch$")
  (#eq? @_method_key "method"))

; ── fetch, verb-less form: `fetch("/users")` is a GET by the WHATWG Fetch
; standard (§ request method, default `GET`) — a specification fact, not a guess.
;
; The trailing `.` anchor makes this pattern match a call with EXACTLY one
; argument, so it is disjoint by construction from the method-bearing pattern
; above: `fetch("/users", {method: "POST"})` can never also emit a phantom GET.
;
; Ceiling (stated): `fetch("/users", {headers: …})` — an init object carrying no
; `method` key — is also a GET, but "an object lacking a key" is not expressible
; as a tree-sitter pattern, and a pattern matching any second argument would
; double-emit against the method-bearing one. It stays honestly uncaptured.
((call_expression
   function: [(identifier) (member_expression)] @_fetch @invoke.http.method.get
   arguments: (arguments
     .
     (_) @invoke.http.arg
     .))
  (#match? @_fetch "^((window|globalThis|self)\\.)?fetch$"))
