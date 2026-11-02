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
; Gin route groups (S-821, CR-255) compose through the shared group resolver
; (`resolve::framework::group`): see the "Route groups" section below.
;
; Deliberately NOT captured: `mux.Handle` with handler-wrapping expressions
; (`http.HandlerFunc(f)` is not a plain name and stays unproven, NFR-RA-05).
;
; Go has no component concept in the ratified set (FR-FW-02 "where
; applicable") — no component patterns.

; Registration with a provable handler name. `@fw.group.route.receiver` is what
; the verb is called on, resolved through the group facts below; the method name
; is the registration's site (`@fw.route.anchor`), shared by the two patterns of
; one registration. A first argument that is a bare HTTP verb is Gin's
; `Handle("GET", "/items", h)`, whose path is its second argument: it is no
; path-first registration, and under a group it would join into `/api/GET`.
((call_expression
  function: (selector_expression
    operand: (_) @fw.group.route.receiver @fw.group.root.default
    field: (field_identifier) @fw.route.method @fw.route.anchor)
  arguments: (argument_list
    .
    [(interpreted_string_literal) (raw_string_literal)] @fw.route.path @fw.route.pattern
    .
    [(identifier) (selector_expression) @fw.route.handler.value] @fw.route.handler))
  (#not-match? @fw.route.path "^[\"`](GET|POST|PUT|DELETE|PATCH|HEAD|OPTIONS|CONNECT|TRACE)[\"`]$"))

; …and the handler-less form (closures, wrapped handlers): the route node is
; still promoted, with no fabricated edge (NFR-RA-05).
((call_expression
  function: (selector_expression
    operand: (_) @fw.group.route.receiver @fw.group.root.default
    field: (field_identifier) @fw.route.method @fw.route.anchor)
  arguments: (argument_list
    .
    [(interpreted_string_literal) (raw_string_literal)] @fw.route.path @fw.route.pattern))
  (#not-match? @fw.route.path "^[\"`](GET|POST|PUT|DELETE|PATCH|HEAD|OPTIONS|CONNECT|TRACE)[\"`]$"))

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

; ── Route groups (S-821, CR-255; see `resolve::framework::group`) ───────────
; `api := r.Group("/api")` then `api.GET("/items", h)` serves `GET /api/items`;
; so do the chain `r.Group("/api").GET("/items", h)`, a nested
; `api.Group("/v1")` (`/api/v1/…`) and the `{ … }` block Gin code puts a
; group's routes in. Gin joins a group's path onto its parent's from the root
; (`r.Group("api")` is `/api`, `@fw.group.rooted`) and keeps a relative path's
; trailing slash, so `api.GET("/")` is `GET /api/`: Go does not opt into
; `@fw.group.absorbs`.
;
; `@fw.group.root.default` (on every registration's receiver) makes Go's default
; a plain router: a receiver whose chain crosses no `Group` call keeps its
; written path — `r := gin.Default()`, an `*gin.Engine` parameter, a field such
; as `s.router`, `http.HandleFunc`, a `ServeMux`. Only a group makes a receiver
; unknown, and a registration on one is refused and counted
; (`routes_not_composed`), never promoted at a partial path:
;   * a `Group` whose path is not a written string literal (`r.Group(prefix)`);
;   * a parameter typed `*gin.RouterGroup`, which may be any group handed in,
;     a `range` variable or a type switch's alias, which may be any value;
;   * a name assigned a group or a value the file cannot read (`api =
;     r.Group(…)`, `api = r.Group(…).Use(…)`, the fourth or later name of one
;     `=`), which is whatever it was last given — an assignment re-binds the
;     variable its own position sees, so one on another function's `r` says
;     nothing about this one;
;   * a call on a group that is not one of the listed methods returning that
;     same group (`@fw.group.foreign`: it may return anything).
;
; Stated ceilings:
;   * read as a plain router, so promoted at the written path: a group kept in
;     a field (`s.api.GET(…)`), returned by a helper (`newGroup().GET(…)`), or
;     bound where no pattern below binds a name (a variadic parameter, the
;     fourth or later name of one `:=`); a parameter typed `gin.IRouter`, which
;     may be the engine or a group;
;   * Gin's `Handle("GET", "/p", h)` promotes nothing: its path is its second
;     argument, which these path-first patterns do not read;
;   * a group handed to another file's function (`func Register(rg
;     *gin.RouterGroup)` called with `r.Group("/api")`) is refused there, as a
;     parameter: the call-argument join is out of scope.

; Every call on a selector is a link of a receiver chain whose result the file
; does not know: on a plain router it is a plain router (`gin.Default()`), on a
; group it is unknown. The patterns after this one read more about the calls
; they match, and the one that read the most wins (`group::Capture::finish`).
(call_expression
  function: (selector_expression
    operand: (_) @fw.group.receiver
    field: (field_identifier) @fw.group.foreign)) @fw.group.call

; A method that returns the group it was called on (Gin's `returnObj`): the
; middleware `Use` and the registrations themselves.
((call_expression
  function: (selector_expression
    operand: (_) @fw.group.receiver
    field: (field_identifier) @fw.group.method)) @fw.group.call
  (#any-of? @fw.group.method
    "Use" "GET" "POST" "PUT" "DELETE" "PATCH" "HEAD" "OPTIONS" "Any" "Handle" "Match"))

; `Group` adds a prefix: its first argument is read as a written literal when it
; is one, otherwise as one the pass cannot read.
((call_expression
  function: (selector_expression
    operand: (_) @fw.group.receiver
    field: (field_identifier) @fw.group.method @fw.group.rooted)
  arguments: (argument_list
    .
    (_) @fw.group.path.opaque)) @fw.group.call
  (#eq? @fw.group.method "Group"))

((call_expression
  function: (selector_expression
    operand: (_) @fw.group.receiver
    field: (field_identifier) @fw.group.method @fw.group.rooted)
  arguments: (argument_list
    .
    [(interpreted_string_literal) (raw_string_literal)] @fw.group.path)) @fw.group.call
  (#eq? @fw.group.method "Group"))

; A local bound to an initializer — `api := r.Group("/api")`, `var v1 =
; api.Group("/v1")` — visible from its name to the end of the statements it is
; declared among. The first three names of one declaration pair with the first
; three values.
(statement_list
  (short_var_declaration
    left: (expression_list . (identifier) @fw.group.name)
    right: (expression_list . (_) @fw.group.init))) @fw.group.scope

(statement_list
  (short_var_declaration
    left: (expression_list . (_) . (identifier) @fw.group.name)
    right: (expression_list . (_) . (_) @fw.group.init))) @fw.group.scope

(statement_list
  (short_var_declaration
    left: (expression_list . (_) . (_) . (identifier) @fw.group.name)
    right: (expression_list . (_) . (_) . (_) @fw.group.init))) @fw.group.scope

; A name an `if` or `switch` initializer binds — `if g := r.Group("/x"); ok {…}`
; — is visible in that statement; one a `for` clause binds, across the loop.
; The first two names pair with the first two values.
(_
  initializer: (short_var_declaration
    left: (expression_list . (identifier) @fw.group.name)
    right: (expression_list . (_) @fw.group.init))) @fw.group.scope

(_
  initializer: (short_var_declaration
    left: (expression_list . (_) . (identifier) @fw.group.name)
    right: (expression_list . (_) . (_) @fw.group.init))) @fw.group.scope

(for_statement
  (for_clause
    initializer: (short_var_declaration
      left: (expression_list . (identifier) @fw.group.name)
      right: (expression_list . (_) @fw.group.init)))) @fw.group.scope

(for_statement
  (for_clause
    initializer: (short_var_declaration
      left: (expression_list . (_) . (identifier) @fw.group.name)
      right: (expression_list . (_) . (_) @fw.group.init)))) @fw.group.scope

; A type switch's alias (`switch g := x.(type)`) is whatever the value turns out
; to be: bound to nothing the pass can read.
(type_switch_statement
  alias: (expression_list (identifier) @fw.group.name)) @fw.group.scope

(statement_list
  (var_declaration
    (var_spec
      name: (identifier) @fw.group.name
      value: (expression_list . (_) @fw.group.init)))) @fw.group.scope

(statement_list
  (var_declaration
    (var_spec_list
      (var_spec
        name: (identifier) @fw.group.name
        value: (expression_list . (_) @fw.group.init))))) @fw.group.scope

; A package-level variable is visible across the file, before its declaration
; as after it.
(source_file
  (var_declaration
    (var_spec
      name: (identifier) @fw.group.name @fw.group.hoisted
      value: (expression_list . (_) @fw.group.init)))) @fw.group.scope

(source_file
  (var_declaration
    (var_spec_list
      (var_spec
        name: (identifier) @fw.group.name @fw.group.hoisted
        value: (expression_list . (_) @fw.group.init))))) @fw.group.scope

; A name assigned after its declaration is whatever it was last given: every
; name on the left of an `=` is marked, and the first three are paired with
; their values. A name only ever given plain routers (`router = gin.New()`) is
; read as its declaration says; one given a group, or a value this file cannot
; read, is unknown wherever that same variable is in scope.
(assignment_statement
  left: (expression_list (identifier) @fw.group.assigned)
  operator: "=")

(assignment_statement
  left: (expression_list . (identifier) @fw.group.assigned)
  operator: "="
  right: (expression_list . (_) @fw.group.assigned.value))

(assignment_statement
  left: (expression_list . (_) . (identifier) @fw.group.assigned)
  operator: "="
  right: (expression_list . (_) . (_) @fw.group.assigned.value))

(assignment_statement
  left: (expression_list . (_) . (_) . (identifier) @fw.group.assigned)
  operator: "="
  right: (expression_list . (_) . (_) . (_) @fw.group.assigned.value))

; A parameter typed as a router group: whichever group the caller chose.
((_
  parameters: (parameter_list
    (parameter_declaration
      (identifier) @fw.group.name
      type: (_) @fw.group.type))) @fw.group.scope
  (#match? @fw.group.type "^[*]?([A-Za-z_][A-Za-z0-9_]*[.])?RouterGroup$"))

; Every other parameter, and a method's receiver, is a plain router — and a
; binding of its own, so a group of the same name outside the function is not
; what it names inside.
((_
  parameters: (parameter_list
    (parameter_declaration
      (identifier) @fw.group.name @fw.group.root
      type: (_) @fw.group.type))) @fw.group.scope
  (#not-match? @fw.group.type "^[*]?([A-Za-z_][A-Za-z0-9_]*[.])?RouterGroup$"))

(method_declaration
  receiver: (parameter_list
    (parameter_declaration
      (identifier) @fw.group.name @fw.group.root))) @fw.group.scope

; A `range` variable is whichever element the loop is at: bound to nothing the
; pass can read.
(for_statement
  (range_clause
    left: (expression_list (identifier) @fw.group.name))) @fw.group.scope
