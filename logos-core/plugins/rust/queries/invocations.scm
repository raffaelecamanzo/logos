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
; Only the receiver-method idiom (`client.get("/p")`, `reqwest::Client::new()
; .get("/p")`) is captured; a free-function form (`reqwest::get("/p")`) is a
; documented coverage ceiling, reported unbound rather than worked around
; (ADR-54).
;
; ── Candidacy is the FILE, not the receiver — a stated ceiling (S-404, CR-128)
;
; The receiver above is `(field_expression field: …)` with NO constraint on the
; operand, so candidacy is decided entirely by the upstream ledger gate
; (`capture_http_client_call_arm`): these anchors are evaluated in any file
; referencing one of the `http_client_detectors` crates, and inside such a file
; EVERY `<anything>.get("/literal")` is captured. `headers.get(…)`,
; `router.get(req.method())` and `params.get(…)` are all promoted to
; `http-client-call` references.
;
; This is the defect shape FR-WS-08 AC5 forbids and S-375 (Java) and S-402 (Go)
; closed for their arms. It is NOT closed here, and until S-404 it was not even
; stated: Rust is the arm the other five were ported FROM, and its silence
; propagated to all of them (CR-128 §3.1).
;
; MEASURED (S-404, 2026-09-15, ~/.cargo/registry/src, 17367 files, 85 admitted
; by the ledger gate): **118 of 293 captured sites (40%)** are on a receiver no
; client-name rule accepts, and **99 of them are unambiguously not HTTP calls**
; — 54 header maps, 11 field bags, 6 parsed JSON bodies, 5 param maps, a
; `HashMap` router lookup, three axum route registrations. The remaining 19 are
; ambiguous (`self`, `store`, `inner`) and at least two of those ARE genuine
; calls, so the honest figure is a range and both ends are recorded. The same
; three shapes dominate as in Go, at 40% against Go's 70% — lower outside a
; gateway, as CR-128 §7 predicted, and still material. The full census and the
; per-language verdicts live in
; `logos-core/tests/operand_resolvability/client_call_gate_finding.txt`, which
; also records why the FILE denominator moves between runs (the cargo registry
; is a live cache) while every other figure here does not.
;
; Over that corpus the arm WROTE 69 `http-client-call` references and 130
; keyless S-374 refusal rows. Both are harmed by the file grain and they are
; harmed differently: a non-HTTP site whose argument is an absolute-path
; literal fabricates a cross-service REFERENCE (NFR-RA-05), one whose argument
; is anything else inflates the `base-url-runtime` DENOMINATOR (FR-WS-05)
; without inventing an edge. The pin below asserts both.
;
; The ceiling is PINNED, not merely written down here, by
; `operand_resolvability::client_call_gate::
;  a_non_client_receiver_call_inside_a_reqwest_file_is_a_stated_ceiling`, whose
; gate-isolating other half is `a_route_shaped_get_outside_a_reqwest_file_is_
; not_captured`. Two tests rather than one, because S-402's trap is that a
; receiver rule refusing the fixture's receiver silences both halves together
; (CR-128 §4.4); the positive control is the bare word `client`, which both
; shipped receiver rules accept WHOLE, so a port moves the ceiling half and
; leaves the control standing.
;
; A port to a receiver-NAME boundary rule over FR-WS-08's Rust row is justified
; by the measurement above and is NOT done here: S-404 is CR-128's measurement
; gate, and the narrowing is the delivery story's to write, with the site-count
; drop recorded as a precision correction per CR-110.
;
; Like every capability query this file is droppable-on-disk: a copy at
; `.logos/plugins/rust/queries/invocations.scm` shadows it without a rebuild
; (FR-PL-04, FR-PL-05).

(call_expression
  function: (field_expression
    field: (field_identifier) @invoke.http.method)
  arguments: (arguments
    .
    (_) @invoke.http.arg))
