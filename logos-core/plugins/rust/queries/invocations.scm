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
; (ADR-54). Like every capability query this file is droppable-on-disk: a copy at
; `.logos/plugins/rust/queries/invocations.scm` shadows it without a rebuild
; (FR-PL-04, FR-PL-05).

(call_expression
  function: (field_expression
    field: (field_identifier) @invoke.http.method)
  arguments: (arguments
    .
    (_) @invoke.http.arg))
