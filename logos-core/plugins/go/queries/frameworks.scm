; Go framework-extraction query (S-015, capability = "frameworks") — the
; ratified set: net/http + Gin (FR-FW-03).
;
; Declarative capture contract (resolve::framework::generic_match): see the
; Python query's header for the capture vocabulary. Droppable on disk at
; `.logos/plugins/go/queries/frameworks.scm`.
;
; Both net/http (`http.HandleFunc("/users", listUsers)`) and Gin
; (`r.GET("/users", listUsers)`) register through a selector call whose first
; argument is the path string — one shape covers both; [framework_methods]
; maps the selector name (`HandleFunc` → ANY, `GET` → GET) and drops
; everything else (FR-FW-04 best-effort).
;
; Deliberately NOT captured in v1: `mux.Handle` with handler-wrapping
; expressions (`http.HandlerFunc(f)` is not a plain name and stays unproven,
; NFR-RA-05), Gin route groups (`g.Group("/api")` prefixes are not joined).
;
; Go has no component concept in the ratified set (FR-FW-02 "where
; applicable") — no component patterns.

; Registration with a provable handler name.
(call_expression
  function: (selector_expression
    field: (field_identifier) @fw.route.method)
  arguments: (argument_list
    .
    [(interpreted_string_literal) (raw_string_literal)] @fw.route.path
    .
    [(identifier) (selector_expression)] @fw.route.handler))

; …and the handler-less form (closures, wrapped handlers): the route node is
; still promoted, with no fabricated edge (NFR-RA-05).
(call_expression
  function: (selector_expression
    field: (field_identifier) @fw.route.method)
  arguments: (argument_list
    .
    [(interpreted_string_literal) (raw_string_literal)] @fw.route.path))
