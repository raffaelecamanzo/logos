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
    [(interpreted_string_literal) (raw_string_literal)] @fw.route.path @fw.route.pattern
    .
    [(identifier) (selector_expression) @fw.route.handler.value] @fw.route.handler))

; …and the handler-less form (closures, wrapped handlers): the route node is
; still promoted, with no fabricated edge (NFR-RA-05).
(call_expression
  function: (selector_expression
    field: (field_identifier) @fw.route.method)
  arguments: (argument_list
    .
    [(interpreted_string_literal) (raw_string_literal)] @fw.route.path @fw.route.pattern))

; ── Method-value handlers (S-526, CR-172, FR-FW-03) ──────────────────────────
; `r.GET("/pull", s.PullHandler)` registers a method of the receiver. The
; handler's selector text (`s.PullHandler` → `s::PullHandler`) binds only through
; the receiver of the method the registration sits in, so the engine needs facts
; a flat match cannot see:
;
;   @fw.receiver.scope / @fw.receiver.name — a method declaration and the
;                 identifier its receiver parameter binds the instance to
;                 (`func (s *Server) …` → `s`). A handler whose head equals the
;                 name of the receiver of the declaration it is written in is
;                 called on that method's own instance and binds among the
;                 receiver type's methods, as `s.PullHandler()` does there
;                 (S-509's recorded self type). An unnamed receiver
;                 (`func (*Server)`) captures nothing, so nothing inside it is
;                 the receiver; a blank one (`func (_ Server)`) captures `_`,
;                 which no handler's head can equal.
;   @fw.route.handler.value — a selector in the handler position of a
;                 registration. Its presence opts the file into counting a
;                 method-value handler whose receiver the file does not prove
;                 (`FrameworkStats::handlers_receiver_unproven`): a parameter
;                 (`srv.Pull`) or a field chain (`s.deps.Pull`) names a value of
;                 unproven type, so the route is promoted with no edge. A
;                 package-qualified handler (`http.NotFound`) is counted by
;                 neither.
;   @fw.route.pattern — the path is a `ServeMux` pattern (`[METHOD ][HOST]/PATH`,
;                 Go 1.22): an all-verbs registration (`Handle`, `HandleFunc`) whose
;                 pattern opens with a standard verb and whitespace takes that verb as
;                 its method, so `"GET /api/status"` is the route `GET /api/status`
;                 and not `ANY GET /api/status`. Without this capture a path is
;                 never split, whatever the dialect.
;
; Known gap, the receiver-shape marker's own (S-517): the head matches the
; receiver by *text*, so a name the method rebinds still reads as the receiver.

(method_declaration
  receiver: (parameter_list
    (parameter_declaration
      name: (identifier) @fw.receiver.name))) @fw.receiver.scope
