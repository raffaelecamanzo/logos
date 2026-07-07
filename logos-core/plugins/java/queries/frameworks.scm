; Java framework-extraction query (S-015, capability = "frameworks") — the
; ratified set: Spring MVC/Boot (FR-FW-03).
;
; Declarative capture contract (resolve::framework::generic_match): see the
; Python query's header for the capture vocabulary. Droppable on disk at
; `.logos/plugins/java/queries/frameworks.scm`.
;
; Deliberately NOT captured in v1: class-level `@RequestMapping` path
; prefixes (method paths are promoted verbatim, not joined), `value =`/
; `path =` annotation arguments (`@GetMapping(value = "/p")`), functional
; `RouterFunction` routing.

; Spring request-mapping annotation on a handler method:
; `@GetMapping("/users") public List<User> list() {…}` — the annotation name
; maps through [framework_methods] (an unmapped annotation promotes nothing).
(method_declaration
  (modifiers
    (annotation
      name: (identifier) @fw.route.method
      arguments: (annotation_argument_list
        (string_literal) @fw.route.path)))
  name: (identifier) @fw.route.handler)

; Spring stereotype class: the wired application building block (FR-FW-02).
; `@fw.component.base` exists only for the predicate.
((class_declaration
  (modifiers
    (marker_annotation
      name: (identifier) @fw.component.base))
  name: (identifier) @fw.component.name)
  (#any-of? @fw.component.base
    "Component" "Service" "Repository" "Controller" "RestController" "Configuration"))
