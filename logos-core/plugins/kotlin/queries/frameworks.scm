; Kotlin framework-extraction query (S-055, capability = "frameworks") — the
; ratified JVM set: Spring MVC/Boot (FR-FW-03), annotation-compatible with the
; Java detector (same `@GetMapping`/stereotype idiom, the same
; [framework_methods] mapping and `org::springframework` candidacy gate).
;
; Declarative capture contract (resolve::framework::generic_match): see the
; Python/Java query headers for the capture vocabulary. Droppable on disk at
; `.logos/plugins/kotlin/queries/frameworks.scm`.
;
; Kotlin annotation shapes differ structurally from Java's: an annotation WITH
; arguments (`@GetMapping("/users")`) parses to a `constructor_invocation`
; (type + value_arguments); a marker annotation (`@RestController`) parses to a
; bare `user_type`. The two patterns below target each shape.
;
; Deliberately NOT captured in v1: class-level `@RequestMapping` path prefixes,
; named `value =`/`path =` annotation arguments, functional `RouterFunction`
; routing.

; Spring request-mapping annotation on a handler function:
; `@GetMapping("/users") fun listUsers() {…}` — the annotation name maps through
; [framework_methods] (an unmapped annotation promotes nothing); the
; string-literal path is unquoted by the resolver.
(function_declaration
  (modifiers
    (annotation
      (constructor_invocation
        (user_type (identifier) @fw.route.method)
        (value_arguments
          (value_argument
            (string_literal) @fw.route.path)))))
  name: (identifier) @fw.route.handler)

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
