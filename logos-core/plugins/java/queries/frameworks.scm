; Java framework-extraction query (S-015, S-328, S-329, capability =
; "frameworks") — the ratified set: Spring MVC/Boot (FR-FW-03, FR-FW-05).
;
; Declarative capture contract (resolve::framework::generic_match): see the
; Python query's header for the capture vocabulary. Droppable on disk at
; `.logos/plugins/java/queries/frameworks.scm`.
;
; This file CAPTURES a class-/interface-level path prefix; it never JOINS one
; onto a method path. Composition — separator normalisation, the prefix-only
; fallback, the non-literal refusal — lives once in the shared interpreter
; (`resolve::framework::compose_prefixes`), which is why the Kotlin query
; inherits every one of those rules by naming the same captures and adding no
; code of its own (S-330).
;
; Deliberately NOT captured: functional `RouterFunction` routing
; (`RouterFunctions.route(GET("/p"), handler)`), whose builder chain names no
; annotated handler method to link; and a *method* path that is not a written
; literal — a constant reference or a concatenation (`value = BASE + "/x"`)
; leaves no literal, so nothing is promoted.
;
; Captured, but NOT interpreted: a property placeholder (`value =
; "${api.base}/x"`) is a written literal, so a method path holding one is
; promoted verbatim; resolving it against the property sources is out of scope
; (FR-FW-05). And in a *mixed* list (`value = {"/a", BASE + "/b"}`) the literal
; elements are promoted while the non-literal ones are dropped silently.
;
; A class-level prefix is stricter, because a prefix is *joined* rather than
; promoted as written: a prefix that is not a resolvable literal path leaves
; every route under it at an unknown address, so those routes are refused
; wholesale (`path-not-composed`, FR-WS-05, NFR-RA-05) instead of promoted at a
; partial path that would falsely claim a provider. `@fw.route.prefix.opaque`
; below is how this file says "a prefix is here and it is not a literal".

; Spring request-mapping annotation on a handler method, positional form:
; `@GetMapping("/users") public List<User> list() {…}` — the annotation name
; maps through [framework_methods] (an unmapped annotation promotes nothing).
(method_declaration
  (modifiers
    (annotation
      name: (identifier) @fw.route.method
      arguments: (annotation_argument_list
        (string_literal) @fw.route.path)) @fw.route.anchor)
  name: (identifier) @fw.route.handler)

; The same annotation in its **named** form — what contract-first code and
; OpenAPI codegen actually emit, and the shape that promoted nothing before
; S-328: `@RequestMapping(method = RequestMethod.GET, value = "/v1/x",
; produces = "application/json")` (FR-FW-05). `value` and `path` are Spring
; aliases, so both key names count; either may hold a list, and
; `value = {"/a", "/b"}` registers one route per element. Writing both keys on
; one annotation is a Spring configuration error (`@AliasFor` conflict); the
; query promotes each independently rather than adjudicating it.
;
; The method declaration is matched the same way whether it carries a body or
; not, so a mapping declared on an **interface** method is captured and its
; bare `@RestController` implementation adds no second route.
;
; On named-vs-positional precedence: both patterns capture `@fw.route.anchor`
; so the interpreter *can* rank them, but for Java the rank never decides
; anything. `annotation_argument_list` is one positional value OR a list of
; named pairs — never both — so mixing them is not legal Java, and the parser
; puts the LEADING argument in an `ERROR` node that neither pattern reaches
; through. `@X("/a", value = "/b")` therefore promotes `/b` and
; `@X(value = "/b", "/a")` promotes `/a`: whichever argument the recovery left
; well-formed, not whichever is named. The precedence pass earns its keep in
; Kotlin, whose homogeneous `value_arguments` list parses the mixed form
; cleanly and does match both patterns at one site (S-330).
((method_declaration
  (modifiers
    (annotation
      name: (identifier) @fw.route.method
      arguments: (annotation_argument_list
        (element_value_pair
          key: (identifier) @fw.route.key
          value: [
            (string_literal) @fw.route.path.named
            (element_value_array_initializer
              (string_literal) @fw.route.path.named)
          ]))) @fw.route.anchor)
  name: (identifier) @fw.route.handler)
  (#any-of? @fw.route.key "value" "path"))

; A mapping annotation carrying NO path at all — `@GetMapping` on a handler
; whose full path is its declaring type's prefix (FR-FW-05, S-329). The
; **marker** form only: `@GetMapping` parses to `marker_annotation`, a node kind
; neither path-bearing pattern above can match, so this pattern never competes
; for a registration site with them — the anchor-pairing invariant holds
; without the interpreter having to rank anything. (`@GetMapping()`, with empty
; parens, parses as an argument-bearing `annotation`; a pattern for it could not
; be constrained to an *empty* argument list and would shadow both patterns
; above, so the degenerate form stays uncaptured.)
;
; Two things keep this broad pattern from manufacturing routes. The
; [framework_methods] gate runs first, so `@Override` — the whole body of a
; bare `@RestController` implementing a prefixed interface — promotes nothing
; because its name is not in the table. And a pathless candidate that clears
; the gate still establishes a route only where a literal prefix is in scope:
; the interpreter drops it otherwise rather than promoting an empty path.
(method_declaration
  (modifiers
    (marker_annotation
      name: (identifier) @fw.route.method) @fw.route.anchor)
  name: (identifier) @fw.route.handler)

; ── Class-/interface-level `@RequestMapping` prefix (S-329, FR-FW-05) ────────
;
; Only `@RequestMapping` prefixes a *type* in Spring — `@GetMapping` and its
; siblings are method-level — so every pattern pins the name. The captures:
;
;   @fw.route.prefix        — the prefix's string literal (unquoted by the pass);
;   @fw.route.prefix.scope  — the declaration whose byte range the prefix
;                             governs. Every route registered inside that range
;                             composes against it, and the *innermost*
;                             containing scope wins, so a prefixed nested class
;                             takes its own prefix and not its enclosing type's
;                             (Spring's rule);
;   @fw.route.prefix.opaque — an argument in the prefix position that is NOT a
;                             written literal. Its mere presence marks the scope
;                             non-composable (the `path_dynamic` idiom of
;                             `resolve::http_client_call`);
;   @fw.route.prefix.name / @fw.route.prefix.key — predicate-only.
;
; A type may declare several literal prefixes (`value = {"/a", "/b"}`); the
; interpreter fans each route out over them. A scope that captures both a
; literal and an opaque argument composes on the literal — the same
; "promote what is established, drop the rest" rule a mixed method-path list
; already follows.

; Every type declaration is a prefix **boundary**, whether or not it declares
; one. A nested type is its own controller bean in Spring and does NOT inherit
; its enclosing type's `@RequestMapping` prefix, so without this pattern a
; handler in an unannotated inner class of a prefixed outer class would be
; promoted at a prefix it is not actually served under. A boundary that
; declares no prefix reads as "unprefixed" — never as a refusal — because it
; ties with, and is unioned into, the prefix patterns below when the same
; declaration matched both.
;
; Records and enums are listed as boundaries but NOT as prefix declarers: a
; prefix *on* one is outside FR-FW-05's "class- or interface-level" scope, and
; the asymmetry is deliberately the safe direction — a handler in a record
; nested in a prefixed class loses a prefix it might have had rather than
; gaining one it never did.
[(class_declaration)
 (interface_declaration)
 (record_declaration)
 (enum_declaration)] @fw.route.prefix.scope

; Positional literal prefix: `@RequestMapping("/v1")`.
([(class_declaration
    (modifiers
      (annotation
        name: (identifier) @fw.route.prefix.name
        arguments: (annotation_argument_list
          (string_literal) @fw.route.prefix))))
  (interface_declaration
    (modifiers
      (annotation
        name: (identifier) @fw.route.prefix.name
        arguments: (annotation_argument_list
          (string_literal) @fw.route.prefix))))] @fw.route.prefix.scope
  (#eq? @fw.route.prefix.name "RequestMapping"))

; Named literal prefix: `@RequestMapping(value = "/v1", produces = ...)`, and
; its list form. `value`/`path` are Spring aliases, so both keys count — and the
; predicate is what stops a sibling string argument (`produces = "text/plain"`)
; from being read as a prefix.
([(class_declaration
    (modifiers
      (annotation
        name: (identifier) @fw.route.prefix.name
        arguments: (annotation_argument_list
          (element_value_pair
            key: (identifier) @fw.route.prefix.key
            value: [
              (string_literal) @fw.route.prefix
              (element_value_array_initializer
                (string_literal) @fw.route.prefix)
            ])))))
  (interface_declaration
    (modifiers
      (annotation
        name: (identifier) @fw.route.prefix.name
        arguments: (annotation_argument_list
          (element_value_pair
            key: (identifier) @fw.route.prefix.key
            value: [
              (string_literal) @fw.route.prefix
              (element_value_array_initializer
                (string_literal) @fw.route.prefix)
            ])))))] @fw.route.prefix.scope
  (#eq? @fw.route.prefix.name "RequestMapping")
  (#any-of? @fw.route.prefix.key "value" "path"))

; Positional NON-literal prefix: `@RequestMapping(BASE)`,
; `@RequestMapping(Paths.V1)`, `@RequestMapping(BASE + "/v1")`. A positional
; argument to `@RequestMapping` is by definition the path, so no key predicate
; applies — and a *named* non-path argument (`method = RequestMethod.GET`) is an
; `element_value_pair`, never a direct child here, so it can never be mistaken
; for a prefix.
([(class_declaration
    (modifiers
      (annotation
        name: (identifier) @fw.route.prefix.name
        arguments: (annotation_argument_list
          [(identifier) (field_access) (binary_expression)] @fw.route.prefix.opaque))))
  (interface_declaration
    (modifiers
      (annotation
        name: (identifier) @fw.route.prefix.name
        arguments: (annotation_argument_list
          [(identifier) (field_access) (binary_expression)] @fw.route.prefix.opaque))))] @fw.route.prefix.scope
  (#eq? @fw.route.prefix.name "RequestMapping"))

; Named NON-literal prefix: `@RequestMapping(value = BASE)`,
; `@RequestMapping(path = BASE + "/v1")`. The array form is listed so a
; part-literal list (`value = {"/a", BASE}`) marks the scope too; the literal
; pattern above then wins the composition.
([(class_declaration
    (modifiers
      (annotation
        name: (identifier) @fw.route.prefix.name
        arguments: (annotation_argument_list
          (element_value_pair
            key: (identifier) @fw.route.prefix.key
            value: [
              (identifier)
              (field_access)
              (binary_expression)
              (element_value_array_initializer)
            ] @fw.route.prefix.opaque)))))
  (interface_declaration
    (modifiers
      (annotation
        name: (identifier) @fw.route.prefix.name
        arguments: (annotation_argument_list
          (element_value_pair
            key: (identifier) @fw.route.prefix.key
            value: [
              (identifier)
              (field_access)
              (binary_expression)
              (element_value_array_initializer)
            ] @fw.route.prefix.opaque)))))] @fw.route.prefix.scope
  (#eq? @fw.route.prefix.name "RequestMapping")
  (#any-of? @fw.route.prefix.key "value" "path"))

; Spring stereotype class: the wired application building block (FR-FW-02).
; `@fw.component.base` exists only for the predicate.
((class_declaration
  (modifiers
    (marker_annotation
      name: (identifier) @fw.component.base))
  name: (identifier) @fw.component.name)
  (#any-of? @fw.component.base
    "Component" "Service" "Repository" "Controller" "RestController" "Configuration"))
