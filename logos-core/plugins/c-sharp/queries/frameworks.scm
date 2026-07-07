; C# framework-extraction query (S-057, CR-009, capability = "frameworks") — the
; ratified set: ASP.NET Core (FR-FW-03).
;
; Declarative capture contract (resolve::framework::generic_match): see the
; Python query's header for the capture vocabulary. Droppable on disk at
; `.logos/plugins/c-sharp/queries/frameworks.scm`.
;
; Deliberately NOT captured in v1: class-level `[Route("[controller]")]` path
; prefixes (method paths are promoted verbatim, not joined), conventional/minimal
; routing (`app.MapGet(...)`), `[Route]` template tokens (`{id}`).

; ASP.NET Core HTTP-verb attribute on an action method:
; `[HttpGet("/users")] public IActionResult List() {…}` — the attribute name maps
; through [framework_methods] (an unmapped attribute promotes nothing).
(method_declaration
  (attribute_list
    (attribute
      name: (identifier) @fw.route.method
      (attribute_argument_list
        (attribute_argument
          (string_literal) @fw.route.path))))
  name: (identifier) @fw.route.handler)

; ASP.NET Core controller class: an `[ApiController]`-attributed type is the
; wired application building block (FR-FW-02). `@fw.component.base` exists only
; for the predicate.
((class_declaration
  (attribute_list
    (attribute
      name: (identifier) @fw.component.base))
  name: (identifier) @fw.component.name)
  (#any-of? @fw.component.base "ApiController"))
