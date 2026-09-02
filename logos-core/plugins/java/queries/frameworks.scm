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
; not written literals — a constant reference or concatenation
; (`value = BASE + "/x"`) leaves no literal to promote, and a property
; placeholder (`value = "${api.base}/x"`) is promoted verbatim, never resolved
; against the property sources.

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
; aliases, so both key names count and neither outranks the other; either may
; hold a list, and `value = {"/a", "/b"}` registers one route per element.
; A named path outranks a positional literal on the same annotation — the
; interpreter ranks them by `@fw.route.anchor`.
;
; The method declaration is matched the same way whether it carries a body or
; not, so a mapping declared on an **interface** method is captured and its
; bare `@RestController` implementation adds no second route.
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
