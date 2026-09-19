; Rust HTTP client-call capture (S-252, capability = "invocations", FR-WS-08).
;
; Captures the *anchors* of an outbound HTTP client call — a method call
; `<receiver>.<method>(<first-arg>, …)` — and leaves every judgment to the
; generic dispatch (`extract::collect_invocation_sites`) and the arm's normalizer
; (`resolve::http_client_call`), exactly as the framework query captures broad
; anchors the framework pass refines. A tree-sitter query cannot decide whether
; the method is an HTTP verb or whether the first argument is a static path
; literal, so those checks live in code:
;
;   @invoke.http.method — the called method identifier (`get`, `post`, …). Kept
;                         only when it is one of the HTTP verbs.
;   @invoke.http.arg    — the call's first argument. Kept as a `"METHOD /template"`
;                         reference only when it is a static string literal; a
;                         bare variable / `format!` / concatenation is refused as
;                         base-url-runtime (never approximately matched, NFR-RA-05).
;
; A `@_`-prefixed capture is ignored by the dispatch (its match arm falls
; through), so it is free to use for predicate operands.
;
; Only the receiver-method idiom (`client.get("/p")`, `reqwest::Client::new()
; .get("/p")`) is captured; a free-function form (`reqwest::get("/p")`) is a
; documented coverage ceiling, reported unbound rather than worked around
; (ADR-54).
;
; ── Candidacy: the RECEIVER, not the file (S-423, CR-128, FR-WS-08 AC5) ─────
;
; Candidacy is ledger-gated upstream (`extract::capture_http_client_call_arm`):
; these anchors are evaluated only in a file referencing one of the
; `http_client_detectors` crates. That gate is a cheap pre-filter and nothing
; more, and the per-call half belongs here.
;
; THE CLAIM THIS SECTION REPLACES, SHOWN AS RETIRED RATHER THAN DELETED
; (ADR-54). Until S-423 this header read "Candidacy is the FILE, not the
; receiver — a stated ceiling", and the receiver below was `(field_expression
; field: …)` with NO constraint on the operand: inside a gate-admitted file
; EVERY `<anything>.get("/literal")` was captured, so `headers.get(…)`,
; `router.get(req.method())` and `params.get(…)` were all promoted to
; `http-client-call` references. Rust is the arm the other five were ported
; FROM, and its silence propagated to all of them (CR-128 §3.1). That claim is
; no longer true of this file; it is kept here because the measurement below,
; and the counts at the end of this section, are only readable against it.
;
; MEASURED (S-404, 2026-09-15, ~/.cargo/registry/src, 17367 files, 85 admitted
; by the ledger gate): **118 of 293 captured sites (40%)** were on a receiver no
; client-name rule accepts, and **99 of them unambiguously not HTTP calls** —
; 54 header maps, 11 field bags, 6 parsed JSON bodies, 5 param maps, a
; `HashMap` router lookup, three axum route registrations. The remaining 19 were
; ambiguous (`self`, `store`, `inner`) and at least two of those ARE genuine
; calls, so the honest figure was a range and both ends are recorded. The same
; three shapes dominate as in Go, at 40% against Go's 70% — lower outside a
; gateway, as CR-128 §7 predicted, and still material. The full census and the
; per-language verdicts live in
; `logos-core/tests/operand_resolvability/client_call_gate_finding.txt`, which
; also records why the FILE denominator moves between runs (the cargo registry
; is a live cache) while every other figure here does not.
;
; Over that corpus the arm WROTE 69 `http-client-call` references and 130
; keyless S-374 refusal rows. Both were harmed by the file grain and they were
; harmed differently: a non-HTTP site whose argument is an absolute-path
; literal fabricates a cross-service REFERENCE (NFR-RA-05), one whose argument
; is anything else inflates the `base-url-runtime` DENOMINATOR (FR-WS-05)
; without inventing an edge.
;
; So the decision is made on the RECEIVER, as S-375 made it for Java and S-402
; for Go. The rule needs no receiver TYPING: it is a receiver-NAME boundary rule
; over FR-WS-08's normative Rust row (`reqwest`-class receiver-method calls —
; the six crates `plugin.toml`'s `http_client_detectors` names), spelled in
; Rust's own casing (snake_case bindings and fields, UpperCamel types) rather
; than in Java's or Go's:
;
;   * the bare word `client` WHOLE, and `http` WHOLE. These are the two
;     spellings this arm's own census found dominating the genuine column.
;   * a name derived from the client TYPE or its crate: one STARTING with a
;     type-derived token — `http_client`, or one of `reqwest` / `hyper` /
;     `isahc` / `ureq` / `awc` / `surf` — after an optional `_` field prefix and
;     before an optional digit/`_`-boundary suffix (`http_client`,
;     `_http_client`, `http_client2`, `http_client_v2`, `reqwest_client`); or
;     one ENDING in `_` plus that same token (`api_http_client`,
;     `orders_http_client`).
;     The receiver is read as a bare identifier OR as the FIELD of one
;     `.`-level down, so `self.client.get(url)` and `s.http_client.get(url)` are
;     both admitted while `metadata.additional_fields.get(k)` is not —
;     `additional_fields` carries no client token.
;
; ── The `::` / `.` distinction, which is Rust's own and is pinned ───────────
;
; This is the line S-404's census had to draw before it could count, and it is
; the reason this rule is not a straight transcription of Go's. The two
; separators mean different things, so the receiver is read differently under
; each:
;
;   * `.` accesses a value ON another value, so only the LAST segment names the
;     receiver. `client.headers()` is a HEADER MAP, not a client — the fact that
;     `client` appears in it is worth nothing. Hence the two `.`-side branches
;     below read exactly one name: the bare identifier, or the field one level
;     down. A receiver that is a `.`-CALL (`client.headers().get(k)`,
;     `response.headers().get(k)`, `self.0.read().await.get(k)`) matches neither
;     and is refused — which is what removes the census's largest bucket.
;   * `::` qualifies ONE type path, so EVERY segment of it names the same thing.
;     `reqwest::Client::new()` IS a client: `reqwest` names it and `Client`
;     names it, and `new` is only how it was built. Hence the third branch
;     captures the scoped callee's PATH (`reqwest::Client`, or bare `Client`)
;     and admits it when ANY `::`-bounded segment is a client name. This branch
;     is what keeps FR-WS-08's Rust row whole: `reqwest::Client::new().get("/p")`
;     is named in this header as a captured idiom and was 3 of the census's
;     genuine sites.
;
; The census was bitten by exactly this before it was drawn: its first run took
; the receiver's last segment unconditionally, so `reqwest::Client::new()`
; reduced to `new()` and three genuine reqwest calls sat in the non-HTTP column
; — a headline of 121 (41%) instead of 118 (40%). Both edges are pinned, there
; by `the_receiver_reduction_handles_each_arms_spelling` and here by
; `the_rust_receiver_rule_reads_a_type_path_whole_and_a_field_chain_last`.
;
; ── Why the bare `Client` suffix is not in the vocabulary ───────────────────
;
; `client` is admitted only WHOLE, and a bare `_client` SUFFIX is deliberately
; absent. This is the Java rule's sharpest line (`ff257427`), adopted by Go
; (`a08e6c6a`) and adopted here for the same reason: the suffix names every
; client protocol in existence, and `cache_client.get("/x")`,
; `redis_client.get("/config/features")` and `zk_client.delete("/config/orders")`
; each clear the verb and absolute-path filters in ordinary Rust — reopening
; CR-110's fabrication class on the consumer side (NFR-RA-05). `client_cache`,
; `client_registry` and `client_store` are the same object spelled the other way
; round, which is why the generic word is whole-only.
;
; It is a BOUNDARY rule, never a substring test: `client_cache`, `clientele`,
; `reqwestcache` and `notareqwest` are all refused, and so is `HttpClientCache`
; on the path side. Pinned on both edges by
; `client_call_gate.rs::the_rust_receiver_rule_is_a_boundary_rule_over_the_
; normative_rust_row`.
;
; ── What this costs, stated rather than worked around (ADR-54) ──────────────
;
; Every one is under-capture, which is the safe direction (NFR-RA-05):
;
;   * A receiver named `self` — `self.delete(uri)` inside an
;     `impl … for reqwest::Client` — is refused. The census found 7 such sites
;     and confirmed at least two are genuine outbound calls
;     (`rmcp-1.7.0/src/transport/common/reqwest/streamable_http_client.rs:98`).
;     A name rule cannot see the impl block; this is Go's single-letter-receiver
;     ceiling in another spelling.
;   * A generically-named wrapper (`api_client`, `orders_client`), a
;     SCREAMING_CASE static (`CLIENT.get("/p")`, the `Lazy<Client>` idiom), and
;     a camelCase receiver (`httpClient`, which is not Rust casing) all stay
;     uncaptured.
;   * A chained receiver — a call, an index, a parenthesized expression
;     (`s.client().get(url)`, `clients[0].get(url)`) — is refused by the operand
;     alternation. `reqwest::Client::builder().build()?.get(url)` is in this
;     class too: its receiver is a `.`-call, so the `::` branch never sees it.
;   * A non-HTTP collaborator SPELLED like a client (`client.get("/admin/users")`
;     on a cache) still captures inside a gate-admitted file. That residual is
;     narrow and named, where the blanket file-grained one was not. It is the
;     positive control of `a_route_shaped_get_outside_a_reqwest_file_is_not_
;     captured`, which is what keeps the FR-FW-04 ledger gate under test after
;     this narrowing (CR-128 §4.4: a fixture whose receiver the new rule refuses
;     stops testing the gate).
;
; ── The count, before and after (a precision correction, CR-110) ────────────
;
; Re-measured through the same harness, same corpus, same machine, both runs on
; 2026-09-19 — so the FILE denominator, which S-404 recorded as unstable because
; the cargo registry is a live cache, did not move between them:
;
;   before  17367 files, 85 gated, 293 sites, 118 (40%) non-HTTP-named,
;           69 references / 130 keyless refusal rows
;   after   17367 files, 85 gated, 175 sites,   0 ( 0%) non-HTTP-named,
;           69 references /  60 keyless refusal rows
;
; 118 sites removed, every one of them on a receiver no client rule accepts.
; The drop is a PRECISION CORRECTION, not a capability loss (CR-110), and the
; two written populations say so from opposite ends:
;
;   * REFERENCES are UNCHANGED at 69. Not one of the 118 removed sites was
;     writing a reference over this corpus, so the narrowing fabricated nothing
;     and un-fabricated nothing here — S-375's "the genuine count does not fall"
;     criterion, met exactly rather than approximately. The fabrication hazard
;     is real (it is what `router.get("/config/features")` models in the pin)
;     but this corpus does not exercise it; that is a fact about the corpus, not
;     a reason to restate the hazard as absent.
;   * REFUSAL ROWS fall 130 -> 60. That is the whole of the measured gain here:
;     70 keyless rows that were inflating the `base-url-runtime` denominator
;     every egress figure is computed over (FR-WS-05) are gone. 70 rows rather
;     than 118, because a refusal row is deduped per DECLARING declaration while
;     a site is not.
;
; The site drop is bounded below by the 99 the census called unambiguously
; non-HTTP and above by the 118 it called non-client-named, and the surviving
; 175 were re-enumerated after the change rather than predicted. The 19
; ambiguous sites are inside the 118 and at least two of them were genuine —
; that is the `self` ceiling above, priced and accepted.
;
; Like every capability query this file is droppable-on-disk: a copy at
; `.logos/plugins/rust/queries/invocations.scm` shadows it without a rebuild
; (FR-PL-04, FR-PL-05).

; ONE pattern, not two, with the three receiver spellings as a node alternation
; binding ONE `@_recv`. The rule therefore has a SINGLE copy. S-375's first cut
; spelled the guard twice and its review had to collapse it (`ff257427`):
; "reverting either copy alone re-admitted `this.perms.get(…)` with the whole
; suite green"; S-402's first cut reproduced that exact state in Go. One copy,
; or the test cannot hold it.
(call_expression
  function: (field_expression
    value: [
      ; `client.get("/p")` — the bare name.
      (identifier) @_recv
      ; `self.client.get("/p")` — the FIELD one `.`-level down, never the whole
      ; chain: only the last segment names the receiver.
      (field_expression
        field: (field_identifier) @_recv)
      ; `reqwest::Client::new().get("/p")` — the scoped callee's type PATH,
      ; read whole, because every `::` segment names the same thing.
      (call_expression
        function: (scoped_identifier
          path: (_) @_recv))
    ]
    field: (field_identifier) @invoke.http.method)
  arguments: (arguments
    .
    (_) @invoke.http.arg)
  (#match? @_recv "^client$|^http$|^_?(http_client|reqwest|hyper|isahc|ureq|awc|surf)([0-9_][a-z0-9_]*)?$|^_?[a-z][a-z0-9_]*_(http_client|reqwest|hyper|isahc|ureq|awc|surf)$|(^|::)(Client|HttpClient|reqwest|hyper|isahc|ureq|awc|surf)(::|$)"))
