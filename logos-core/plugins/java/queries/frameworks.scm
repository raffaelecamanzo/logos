; Java framework-extraction query (S-015, S-328, capability = "frameworks") —
; the ratified set: Spring MVC/Boot (FR-FW-03, FR-FW-05).
;
; Declarative capture contract (resolve::framework::generic_match): see the
; Python query's header for the capture vocabulary. Droppable on disk at
; `.logos/plugins/java/queries/frameworks.scm`.
;
; Deliberately NOT captured: class-level `@RequestMapping` prefixes — a method
; path is promoted verbatim, never joined onto its declaring type's prefix
; (composition is S-329's, in the shared interpreter); functional
; `RouterFunction` routing (`RouterFunctions.route(GET("/p"), handler)`), whose
; builder chain names no annotated handler method to link; and paths that are
; not written literals — a constant reference or a concatenation
; (`value = BASE + "/x"`) leaves no literal, so nothing is promoted.
;
; Captured, but NOT interpreted: a property placeholder (`value =
; "${api.base}/x"`) is a written literal, so it is promoted verbatim; resolving
; it against the property sources is out of scope (FR-FW-05). And in a *mixed*
; list (`value = {"/a", BASE + "/b"}`) the literal elements are promoted while
; the non-literal ones are dropped silently — reporting that as
; `path-not-composed` is S-329's.

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

; Spring stereotype class: the wired application building block (FR-FW-02).
; `@fw.component.base` exists only for the predicate.
((class_declaration
  (modifiers
    (marker_annotation
      name: (identifier) @fw.component.base))
  name: (identifier) @fw.component.name)
  (#any-of? @fw.component.base
    "Component" "Service" "Repository" "Controller" "RestController" "Configuration"))
