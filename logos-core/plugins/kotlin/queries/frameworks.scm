; Kotlin framework-extraction query (S-055, S-330, S-561, capability =
; "frameworks") — the ratified JVM set: Spring MVC/Boot (FR-FW-03, FR-FW-05),
; annotation-compatible with the Java detector: the same `@GetMapping`
; /stereotype idiom, the same annotation rows of `[framework_methods]` and the
; `org::springframework` candidacy gate — plus the two call-shaped routing DSLs
; Kotlin owns: Spring's functional `router`/`coRouter` and Ktor's `routing`
; (see "Functional routing" below), the latter behind an `io::ktor` gate added
; to Kotlin's. The annotation patterns read an annotation by its `…Mapping` name
; as well as by the table, so the DSL's call names (`GET`, `get`) in the same
; table never read as annotations (a Retrofit `@GET("users")`).
;
; Declarative capture contract (resolve::framework::generic_match): see the
; Python query's header for the capture vocabulary. Droppable on disk at
; `.logos/plugins/kotlin/queries/frameworks.scm`.
;
; This file CAPTURES a class-/interface-level path prefix; it never JOINS one
; onto a method path. Composition — separator normalisation, the prefix-only
; fallback, the innermost-scope rule, the non-literal refusal — lives once in
; the shared interpreter (`resolve::framework::compose_prefixes`), so Kotlin
; contributes **no** composition code of its own: the whole per-language
; surface is the capture names below, which is the claim S-330 verifies. The
; functional DSLs' nesting scopes chain instead of nesting innermost-first;
; that rule is the interpreter's too (`resolve::framework::nest`).
;
; ── How Kotlin's annotation tree differs from Java's ─────────────────────────
;
; An annotation WITH arguments (`@GetMapping("/users")`) parses to a
; `constructor_invocation` — a `user_type` plus `value_arguments`. A **marker**
; annotation (`@RestController`, a bare `@GetMapping`) parses to a bare
; `user_type` child instead. The two shapes are structurally disjoint, so a
; marker pattern can never share a registration site with an argument-bearing
; one: the anchor-pairing invariant Java gets from `marker_annotation` versus
; `annotation`, here for the same reason.
;
; A **named** argument is not Java's `element_value_pair`. Kotlin's
; `value_arguments` is a homogeneous list of `value_argument` nodes and a name
; is simply an `identifier` child in front of the value
; (`value_argument: (identifier '=')? expression`). Two consequences this file
; has to handle, neither of them theoretical:
;
;   1. an UNANCHORED `(value_argument (string_literal))` matches the named form
;      too, because the literal is a direct child either way. That is what this
;      query wrote before S-330, so `value = "/x"` already promoted a route —
;      and so did `produces = "application/json"`, promoting a media type as a
;      URL, the approximate match NFR-RA-05 forbids. Every pattern below that
;      reads a *positional* argument therefore says so explicitly: the literal
;      patterns anchor with `.` to the argument's FIRST named child, which is
;      exactly the child a named argument's key occupies, and the opaque
;      patterns — which match the `expression` supertype, and so would match
;      the key itself — compare the value's text against the whole argument's
;      instead (see the opaque section for why the anchor cannot be used
;      there);
;   2. `value_arguments` accepts positional and named arguments at one site, so
;      `@RequestMapping("/a", value = "/b")` parses cleanly where the Java
;      equivalent is illegal and `ERROR`-recovered. Both path patterns
;      therefore capture `@fw.route.anchor`, so `drop_outranked_paths` can
;      arbitrate rather than promoting two routes for one registration. Kotlin
;      is the dialect that precedence pass was built for (S-328).
;
; The anchor is the `constructor_invocation` (or, for a marker, the `user_type`)
; — deliberately NOT the enclosing `annotation` node. Kotlin's bracket form
; `@[GetMapping(value = "/a") PostMapping("/b")]` puts several *independent*
; registrations under one `annotation`, so anchoring there would make
; precedence suppress one sibling's route as if it were a second path on one
; annotation. Anchoring per invocation keeps "one anchor = one registration"
; true in both spellings, and the two anchored node kinds stay disjoint, so a
; marker can still never share a site with an argument-bearing form.
;
; Deliberately NOT captured: the `RouterFunctions.route(…)` builder chain, which
; names no router block to scope a registration to; a *method* path that is not
; a written literal — a constant reference, a concatenation or a string
; template leaves no literal, so nothing is promoted rather than a guessed
; path; and `@GetMapping()` with empty parentheses, a degenerate shape a
; structural pattern alone cannot separate from an argument-bearing annotation
; (Java leaves it uncaptured for the same reason).
;
; ── Out of reach: an upstream grammar defect, not an exclusion ──────────────
;
; `tree-sitter-kotlin-ng` 1.1 fails to parse a type declaration carrying **two
; or more annotations** when another top-level declaration follows it in the
; file — the overwhelmingly common `@RestController @RequestMapping(...) class
; C { … }` plus a DTO, a service interface or a second controller. The
; declaration is not recognised as a `class_declaration` at all: its modifiers
; are reparsed as a file-level expression, `has_error()` stays **false**, and
; no pattern in this file can see it — `symbols.scm` counts the expression into
; the file's partial-extraction warning (S-559). The same defect swallows
; an annotated `enum class` outright unless a second modifier precedes it
; (`internal enum class` parses and composes).
;
; This is NOT introduced by prefix composition — the pre-S-330 query lost the
; route and the stereotype component on the same input, so the defect predates
; it. What composition adds is that in the sub-case where the handler survives
; the misparse, it is promoted at its **bare method path** rather than the
; composed one: an under-composed address rather than an absent one. Escalated
; for a grammar-upgrade change request; `an_upstream_misparse_of_a_multiply_
; annotated_type_loses_its_prefix` pins the behaviour so the upgrade is
; noticed.
;
; Two narrower shapes this file also cannot reach, both parity-consistent with
; Java and both failing safe (a missing route, never a wrong one):
;
;   - a **positional** array method path (`@GetMapping(["/a","/b"])`) is not
;     captured. Java's positional pattern has the same gap for `{...}`, so the
;     two languages agree; Spring codegen writes the named form;
;   - a mapping annotation whose only arguments are non-path
;     (`@GetMapping(produces = ["text/plain"])`) is a `constructor_invocation`,
;     so it matches neither path-bearing pattern (the key predicate rejects it)
;     nor the marker pattern (which requires a bare `user_type`). Under a
;     prefixed type its route would be the prefix alone; it is not promoted.
;     Java behaves identically.
;
; Captured, but NOT interpreted: a property placeholder (`value =
; "${api.base}/x"`) is a written literal, so a *method* path holding one is
; promoted verbatim; resolving it against the property sources is out of scope
; (FR-FW-05). And in a mixed list (`value = ["/a", BASE]`) the literal elements
; are promoted while the rest are dropped silently.
;
; A class-level prefix is stricter, because a prefix is *joined* rather than
; promoted as written: a prefix that is not a resolvable literal leaves every
; route under it at an unknown address, so those routes are refused wholesale
; (`path-not-composed`, FR-WS-05, NFR-RA-05) instead of promoted at a partial
; path that would falsely claim a provider. `@fw.route.prefix.opaque` below is
; how this file says "a prefix is here and it is not a literal". Kotlin string
; templates are caught on their *text* by the shared `is_resolvable_prefix`
; rather than structurally: this grammar gives `"${x}"` an `interpolation`
; child a query could capture, but folds the equally common `"$x"` into two
; plain `string_content` runs with no node to name — and a lone trailing `$`
; (`"/price$"`) folds the same way, so no structural pattern can tell a
; template from a dollar sign. One text rule covers both spellings and keeps
; `/price$` composable.

; ── Handler registrations ────────────────────────────────────────────────────

; Spring request-mapping annotation on a handler function, positional form:
; `@GetMapping("/users") fun listUsers() {…}` — the annotation name maps
; through `[framework_methods]` (an unmapped annotation promotes nothing) and
; the string-literal path is unquoted by the resolver.
;
; The leading `.` is load-bearing: it pins the literal to the argument's first
; named child, which is what separates a positional argument from
; `value = "/x"` (whose first named child is the key `identifier`). Without it
; this pattern also matches every string-valued named argument, `produces`
; included.
((function_declaration
  (modifiers
    (annotation
      (constructor_invocation
        (user_type (identifier) @fw.route.method @fw.route.slashed)
        (value_arguments
          (value_argument
            .
            (string_literal) @fw.route.path))) @fw.route.anchor))
  name: (identifier) @fw.route.handler)
  (#match? @fw.route.method "Mapping$"))

; The same annotation in its **named** form — what contract-first code and
; OpenAPI codegen actually emit: `@RequestMapping(method = RequestMethod.GET,
; value = "/v1/x", produces = "application/json")` (FR-FW-05). `value` and
; `path` are Spring aliases, so both key names count; either may hold a list,
; and `value = ["/a", "/b"]` registers one route per element. Writing both keys
; on one annotation is a Spring configuration error (an `@AliasFor` conflict);
; the query promotes each independently rather than adjudicating it.
;
; The function declaration is matched the same way whether it carries a body or
; not, so a mapping declared on an **interface** function is captured and its
; bare `@RestController` implementation adds no second route — in Kotlin the
; implementation carries the `override` *keyword* rather than an annotation, so
; it matches no pattern here at all.
;
; The key predicate is what stops a sibling string argument
; (`produces = "text/plain"`) from being read as a URL.
((function_declaration
  (modifiers
    (annotation
      (constructor_invocation
        (user_type (identifier) @fw.route.method @fw.route.slashed)
        (value_arguments
          (value_argument
            .
            (identifier) @fw.route.key
            [
              (string_literal) @fw.route.path.named
              (collection_literal (string_literal) @fw.route.path.named)
            ]))) @fw.route.anchor))
  name: (identifier) @fw.route.handler)
  (#any-of? @fw.route.key "value" "path" "`value`" "`path`")
  (#match? @fw.route.method "Mapping$"))

; A mapping annotation carrying NO path at all — `@GetMapping` on a handler
; whose full path is its declaring type's prefix (FR-FW-05, S-329). The
; **marker** form only: a marker annotation's child is a bare `user_type`,
; which neither path-bearing pattern above can match, so this pattern never
; competes for a registration site with them.
;
; Two things keep this broad pattern from manufacturing routes. The
; `[framework_methods]` gate runs first, so an unmapped annotation
; (`@Autowired`, `@Test`) promotes nothing. And a pathless candidate that
; clears the gate still establishes a route only where a literal prefix is in
; scope: the interpreter drops it otherwise rather than promoting an empty
; path.
((function_declaration
  (modifiers
    (annotation
      (user_type (identifier) @fw.route.method) @fw.route.anchor))
  name: (identifier) @fw.route.handler)
  (#match? @fw.route.method "Mapping$"))

; ── Class-/interface-level `@RequestMapping` prefix (S-329, S-330) ───────────
;
; Only `@RequestMapping` prefixes a *type* in Spring — `@GetMapping` and its
; siblings are member-level — so every pattern pins the name. The captures are
; the shared ones (`@fw.route.prefix`, `@fw.route.prefix.scope`,
; `@fw.route.prefix.opaque`; `@fw.route.prefix.name` / `.key` / `.value` /
; `.arg` are predicate-only), documented in the Python query's header.
;
; "A type declaration" is expressed as a wildcard node with one of the two
; type-body kinds as a child: `class_body`, which Kotlin's `class`, `interface`
; and `object` all use, or `enum_class_body`. The grammar's `declaration`
; supertype looks like the obvious spelling and is NOT usable — it covers
; `function_declaration` too, so the query engine would make every
; method-level mapping its own prefix and self-compose it into `/v1/x/v1/x`
; (the trap S-329 hit with Java's equivalent supertype).
;
; Kotlin puts a declaration's annotations inside its `modifiers` child, so the
; matched declaration node already spans annotation *and* body — the range the
; interpreter requires. Java needed a wildcard to reach the same span.
;
; A type may declare several literal prefixes (`value = ["/a", "/b"]`); the
; interpreter fans each route out over them. In a mixed list
; (`value = ["/a", BASE]`) the opaque capture overlaps only `BASE`, so `/a`
; still composes.

; Every type declaration is a prefix **boundary**, whether or not it declares
; one. A nested type is its own controller bean in Spring and does NOT inherit
; its enclosing type's `@RequestMapping`, so without this pattern a handler in
; an unannotated inner class of a prefixed outer class would be promoted at a
; prefix it is not actually served under. A boundary that declares no prefix
; reads as "unprefixed" — never as a refusal — because it shares its range
; with, and is combined into, the prefix patterns below.
(_ [(class_body) (enum_class_body)]) @fw.route.prefix.scope

; Positional prefix: `@RequestMapping("/v1")`, `@RequestMapping(["/v1","/v2"])`.
((_ (modifiers
      (annotation
        (constructor_invocation
          (user_type (identifier) @fw.route.prefix.name @fw.route.slashed)
          (value_arguments
            (value_argument
              .
              [
                (string_literal) @fw.route.prefix
                (collection_literal (string_literal) @fw.route.prefix)
              ])))))
    [(class_body) (enum_class_body)]) @fw.route.prefix.scope
  (#eq? @fw.route.prefix.name "RequestMapping"))

; Named prefix: `@RequestMapping(value = "/v1", produces = …)` and its list
; form. `value`/`path` are Spring aliases, so both keys count — and the key
; predicate is what stops `produces = "text/plain"` from relocating every route
; in the class.
((_ (modifiers
      (annotation
        (constructor_invocation
          (user_type (identifier) @fw.route.prefix.name @fw.route.slashed)
          (value_arguments
            (value_argument
              .
              (identifier) @fw.route.prefix.key
              [
                (string_literal) @fw.route.prefix
                (collection_literal (string_literal) @fw.route.prefix)
              ])))))
    [(class_body) (enum_class_body)]) @fw.route.prefix.scope
  (#eq? @fw.route.prefix.name "RequestMapping")
  (#any-of? @fw.route.prefix.key "value" "path" "`value`" "`path`"))

; The same two argument forms in the **opaque** position — the half that makes
; a capture gap fail CLOSED. `(expression)` is the supertype catch-all: it
; matches every non-literal argument form at once with no enumeration to fall
; behind (a `multiline_string_literal` prefix, which the literal patterns
; deliberately do not read, lands here rather than leaving the type looking
; unprefixed). It also matches a `string_literal` in the same position; the
; interpreter ignores an opaque range identical to a captured literal, so the
; literal patterns still win wherever the argument reads.
;
; The first-child anchor separates the positional form here as it does above,
; for a reason worth stating because it is not obvious: `identifier` is a
; declared subtype of the `expression` supertype, so `(expression)` looks like
; it should match a named argument's KEY and refuse the whole controller. It
; does not. The grammar spells the argument as
; `(_identifier '=')? '*'? expression`, and tree-sitter resolves a supertype
; pattern **positionally** — only against the slot the grammar declared as
; `expression`. So `(value_argument (expression) @x)` never binds a key,
; anchored or not, while `(value_argument (string_literal) @x)` (a concrete
; kind) matches anywhere and genuinely needs the `.`. Verified over 155
; argument-form x host-kind combinations.
;
; The two predicates below are both about an **empty** list, which Spring
; reads as "no prefix" and this grammar cannot represent at all — its
; `collection_literal` takes a `commaSep1` element list, so `value = []`
; error-recovers into a `collection_literal` holding one zero-width `MISSING`
; node. `#not-match?` keeps the scalar branch off the bare `[…]`, and
; `#not-eq? ""` keeps the element branch off the artefact inside it; together
; they mean `value = []` declares nothing while `value = [BASE]` still refuses
; through the element branch. Without them a legal annotation would refuse
; every route in the controller because of a parse artefact.
((_ (modifiers
      (annotation
        (constructor_invocation
          (user_type (identifier) @fw.route.prefix.name @fw.route.slashed)
          (value_arguments
            (value_argument
              .
              [
                (expression) @fw.route.prefix.opaque
                (collection_literal (expression) @fw.route.prefix.opaque)
              ])))))
    [(class_body) (enum_class_body)]) @fw.route.prefix.scope
  (#eq? @fw.route.prefix.name "RequestMapping")
  (#not-match? @fw.route.prefix.opaque "^\\[")
  (#not-eq? @fw.route.prefix.opaque ""))

((_ (modifiers
      (annotation
        (constructor_invocation
          (user_type (identifier) @fw.route.prefix.name @fw.route.slashed)
          (value_arguments
            (value_argument
              .
              (identifier) @fw.route.prefix.key
              [
                (expression) @fw.route.prefix.opaque
                (collection_literal (expression) @fw.route.prefix.opaque)
              ])))))
    [(class_body) (enum_class_body)]) @fw.route.prefix.scope
  (#eq? @fw.route.prefix.name "RequestMapping")
  (#any-of? @fw.route.prefix.key "value" "path" "`value`" "`path`")
  (#not-match? @fw.route.prefix.opaque "^\\[")
  (#not-eq? @fw.route.prefix.opaque ""))

; ── Slash-less paths ─────────────────────────────────────────────────────────────
;
; Spring serves every mapping from the root, whether or not it was written with a
; leading slash: `@GetMapping("vets.xml")` is `/vets.xml` (S-561). The
; route-method and prefix-name captures of the annotation patterns above also
; carry `@fw.route.slashed`, which makes the shared interpreter give a written
; path or prefix the slash it left out. An empty path stays empty:
; `@RequestMapping("")` supplies no prefix (S-329). The functional DSL and Ktor
; patterns below carry no such capture: a router block states the root `/`
; (`@fw.route.prefix.root`), which the join the interpreter makes gives every
; slash-less `nest`, `route` or registration path (`"admin".nest { GET("users") }`
; is `/admin/users`, as is Ktor's `route("admin") { get("users") }`).

; ── Functional routing: Spring's `router` / `coRouter` DSL and Ktor's `routing` ─
;
; Both register routes by CALLING functions inside a lambda — `GET("/x", h::m)`,
; `get("/x") { … }` — so a registration is a route only under a router block,
; and a prefix is a `nest` or `route` call around it. Neither names an annotated
; handler, so these patterns capture three things and leave every decision to
; the shared interpreter (`resolve::framework::nest`):
;
;   `@fw.route.prefix.scope` + `@fw.route.prefix.nest` — a lambda that nests: the
;       interpreter joins EVERY nesting scope around a registration, outermost
;       first (a type's `@RequestMapping` is the innermost scope alone);
;   `@fw.route.nested` — a registration that is a route only inside a nesting
;       scope: outside every one it is not a route (`mockMvc.get("/x") { … }`,
;       a cache lookup), and promotes nothing and counts nothing;
;   `@fw.binding.*` — the typed property a handler reference is written on.
;
; A router block (`router`, `coRouter`, `routing`) is a nesting scope with the
; root `/` as its prefix (`@fw.route.prefix.root`).
((call_expression
   (identifier) @fw.route.prefix.name @fw.route.prefix.root
   (annotated_lambda) @fw.route.prefix.scope @fw.route.prefix.nest)
  (#any-of? @fw.route.prefix.name "router" "coRouter" "routing"))

; Spring's `"/admin".nest { … }`: a written literal prefix.
((call_expression
   (navigation_expression
     (string_literal) @fw.route.prefix
     (identifier) @fw.route.prefix.name)
   (annotated_lambda) @fw.route.prefix.scope @fw.route.prefix.nest)
  (#eq? @fw.route.prefix.name "nest"))

; Spring's `(accept(APPLICATION_JSON) and "/api").nest { … }`: the literal beside
; a predicate that names no path. Only those four predicates are read as
; path-free; any other operand is the opaque prefix below.
((call_expression
   (navigation_expression
     (parenthesized_expression
       (infix_expression
         (call_expression (identifier) @fw.route.prefix.key)
         (identifier) @fw.route.prefix.value
         (string_literal) @fw.route.prefix))
     (identifier) @fw.route.prefix.name)
   (annotated_lambda) @fw.route.prefix.scope @fw.route.prefix.nest)
  (#eq? @fw.route.prefix.name "nest")
  (#eq? @fw.route.prefix.value "and")
  (#any-of? @fw.route.prefix.key "accept" "contentType" "headers" "method"))

((call_expression
   (navigation_expression
     (parenthesized_expression
       (infix_expression
         (string_literal) @fw.route.prefix
         (identifier) @fw.route.prefix.value
         (call_expression (identifier) @fw.route.prefix.key)))
     (identifier) @fw.route.prefix.name)
   (annotated_lambda) @fw.route.prefix.scope @fw.route.prefix.nest)
  (#eq? @fw.route.prefix.name "nest")
  (#eq? @fw.route.prefix.value "and")
  (#any-of? @fw.route.prefix.key "accept" "contentType" "headers" "method"))

; Any other receiver of `nest` is a prefix this file cannot read — the opaque
; half, which makes a gap fail CLOSED: its registrations are refused and
; counted, never promoted at the partial path the enclosing scopes give. The
; receivers it excludes are the ones the patterns above read: a bare
; path-free predicate (`accept(TEXT_HTML).nest { … }` adds no segment, so the
; lambda is no scope at all), and the parenthesised `and` of one with a literal.
; The text rule admits one level of parentheses in the predicate's arguments;
; anything more reads as opaque, the safe side. A literal receiver also matches
; here, at exactly the literal's range, and the interpreter ignores an opaque
; capture that is just a captured literal restated.
((call_expression
   (navigation_expression
     . (_) @fw.route.prefix.opaque
     (identifier) @fw.route.prefix.name .)
   (annotated_lambda) @fw.route.prefix.scope @fw.route.prefix.nest)
  (#eq? @fw.route.prefix.name "nest")
  (#not-match? @fw.route.prefix.opaque "^(?:(?:accept|contentType|headers|method)\\((?:[^()]|\\([^()]*\\))*\\)|\\(\\s*(?:accept|contentType|headers|method)\\((?:[^()]|\\([^()]*\\))*\\)\\s+and\\s+\"[^\"\\n]*\"\\s*\\)|\\(\\s*\"[^\"\\n]*\"\\s+and\\s+(?:accept|contentType|headers|method)\\((?:[^()]|\\([^()]*\\))*\\)\\s*\\))$"))

; Ktor's `route("/p") { … }` (the optional second argument, an `HttpMethod`,
; names no path), and the opaque form `route(PATH) { … }`.
((call_expression
   (call_expression
     (identifier) @fw.route.prefix.name
     (value_arguments
       . (value_argument . (string_literal) @fw.route.prefix)))
   (annotated_lambda) @fw.route.prefix.scope @fw.route.prefix.nest)
  (#eq? @fw.route.prefix.name "route"))

((call_expression
   (call_expression
     (identifier) @fw.route.prefix.name
     (value_arguments
       . (value_argument . (expression) @fw.route.prefix.opaque)))
   (annotated_lambda) @fw.route.prefix.scope @fw.route.prefix.nest)
  (#eq? @fw.route.prefix.name "route"))

; A Ktor routing extension — `fun Route.users() { get("/u") { … } }` — is mounted
; wherever another file calls it (`route("/api") { users() }`), which this file
; does not show. Its registrations are refused and counted, never promoted at a
; bare path that would claim a provider at the wrong address.
((function_declaration
   (user_type (identifier) @fw.route.prefix.name) @fw.route.prefix.opaque
   .
   name: (identifier)) @fw.route.prefix.scope @fw.route.prefix.nest
  (#any-of? @fw.route.prefix.name "Route" "Routing"))

; The registrations. The method text maps through `[framework_methods]`: Spring's
; upper-case `GET` and Ktor's lower-case `get` are separate rows, so an unmapped
; call promotes nothing.
;
; Positional path with a handler reference: `GET("/talk", talkHandler::find)`.
; The reference is a `navigation_expression` whose separator is the `::` token
; (a `navigation_expression` with `.` is a property read, never a handler) or a
; bare `callable_reference` (`::find`).
((call_expression
   (identifier) @fw.route.method @fw.route.nested
   (value_arguments
     . (value_argument . (string_literal) @fw.route.path)
     . (value_argument
       [
         (navigation_expression "::") @fw.route.handler
         (callable_reference) @fw.route.handler
       ]))) @fw.route.anchor
  (#not-match? @fw.route.path "\\$[{A-Za-z_]"))

; Positional path, handler written as a lambda or not at all: `GET("/x") { … }`,
; `get("/x") { … }`, and the bare `GET("/x")` of a predicate combination
; (`GET("/a") or GET("/b")`). A path with a string-template reference is not a
; written path — it is the opaque form below — so one `GET("/$year/")` in a loop
; is never promoted as a route whose name holds the template.
((call_expression
   (identifier) @fw.route.method @fw.route.nested
   (value_arguments
     . (value_argument . (string_literal) @fw.route.path))) @fw.route.anchor
  (#not-match? @fw.route.path "\\$[{A-Za-z_]"))

; A path that is not a written literal: counted in `routes_not_composed`, never
; guessed.
(call_expression
  (identifier) @fw.route.method @fw.route.nested
  (value_arguments
    . (value_argument . (expression) @fw.route.path.opaque))) @fw.route.anchor

; Ktor's pathless `get { … }`: the registration is the enclosing prefix itself.
(call_expression
  (identifier) @fw.route.method @fw.route.nested
  (annotated_lambda)) @fw.route.anchor

; A registration's handler body is no router block. The calls inside
; `get("/h") { respond(buildJsonObject { put("status", "ok") }) }` are the
; handler's own code, and kotlinx.html's `head { }` is no `HEAD` registration, so
; the trailing lambda of a call the descriptor maps (`@fw.route.barrier.name` is
; read through `[framework_methods]`) is a barrier: a registration inside one is
; no route and is not counted. The third pattern is the lambda a parenthesised
; predicate combination takes, `(GET("/a") or GET("/b")) { … }`.
(call_expression
  (call_expression (identifier) @fw.route.barrier.name (value_arguments))
  (annotated_lambda) @fw.route.barrier)

(call_expression
  (identifier) @fw.route.barrier.name
  (annotated_lambda) @fw.route.barrier)

(call_expression
  (parenthesized_expression)
  (annotated_lambda) @fw.route.barrier)

; The properties a handler reference can be written on, by their declared type:
; a primary-constructor `val`/`var` and a member property that states its type,
; each visible throughout its class. A type written as one bare name is read; a
; generic, nullable or qualified one is not, so its handlers bind nothing.
; Known gap: the match is the head's *text*, so a lambda parameter or local
; that rebinds the property's name still reads as the property, and so does an
; inner declaration of it whose type is not one bare name (it is not captured,
; so it does not shadow the outer one). The binder
; still has to prove the member (`resolve::framework::nest`).
((class_declaration
   (primary_constructor
     (class_parameters
       (class_parameter
         ["val" "var"]
         (identifier) @fw.binding.name
         (user_type . (identifier) @fw.binding.type .))))) @fw.binding.scope)

((class_declaration
   (class_body
     (property_declaration
       (variable_declaration
         (identifier) @fw.binding.name
         (user_type . (identifier) @fw.binding.type .))))) @fw.binding.scope)

; ── Components ───────────────────────────────────────────────────────────────

; Spring stereotype class: the wired application building block (FR-FW-02).
; `@RestController class UserController` — a marker annotation (no arguments).
; `@fw.component.base` exists only for the predicate.
((class_declaration
  (modifiers
    (annotation
      (user_type (identifier) @fw.component.base)))
  name: (identifier) @fw.component.name)
  (#any-of? @fw.component.base
    "Component" "Service" "Repository" "Controller" "RestController" "Configuration"))
