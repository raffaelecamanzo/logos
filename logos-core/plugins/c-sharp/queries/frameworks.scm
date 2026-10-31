; C# framework-extraction query (S-057, CR-009, S-554, CR-177, capability =
; "frameworks") — the ratified set: ASP.NET Core (FR-FW-03).
;
; Declarative capture contract (resolve::framework::generic_match): see the
; Python query's header for the capture vocabulary; the route-group and token
; captures below are described in `resolve::framework::group`. Droppable on disk
; at `.logos/plugins/c-sharp/queries/frameworks.scm`.
;
; Three registration shapes, each composed by the shared interpreter and none by
; this file (a pattern captures, it cannot concatenate):
;
;   * an attribute route on an action — `[HttpGet("{id}")]`, `[HttpPost]` — joined
;     onto the class's `[Route("api/[controller]")]` prefix;
;   * a minimal-API registration — `app.MapGet("/p", Handler)` — joined onto the
;     prefix of the `MapGroup` its receiver is (through any chain of builder
;     calls and nested groups);
;   * a class prefix itself, a `@fw.route.prefix` match with no route.
;
; ASP.NET Core serves every template from the root whether or not it was written
; with a leading slash (`"api/catalog"`, `"{id}"`), so every path and prefix here
; carries `@fw.route.rooted`. Two captures carry what is ASP.NET's own:
;
;   @fw.route.token.controller — the class a prefix belongs to: `[controller]`
;       in the prefix is its name without the `Controller` suffix. (`[action]` is
;       expanded per route from its handler, wherever it was written; a token
;       left over — `[area]`, a `[controller]` in an action's own path — refuses
;       the route.)
;   @fw.route.absolute — a path written `"/x"` or `"~/x"` (an attribute path or a
;       class prefix) ignores any prefix above it (the shape test is the shared
;       pass's).
;
; Stated ceilings:
;   * a class that inherits its `[Route]` from a base class is promoted at its
;     actions' own paths — the prefix is the base's, which this query does not
;     chase;
;   * a receiver is read as a plain builder only when this file proves it: a
;     parameter typed `IEndpointRouteBuilder` / `WebApplication`, or a chain from
;     the static `WebApplication`. A group built by a helper, handed in as a
;     `RouteGroupBuilder` parameter, bound to a lambda parameter, kept in a
;     member, assigned after its declaration, introduced by a `foreach`/`using`/
;     pattern/`out` variable, or reached through a call that is not a listed
;     fluent builder method, is refused and counted (`routes_not_composed`) —
;     never promoted at a partial path. The one place this over-promotes: a method
;     taking `IEndpointRouteBuilder` is read as a root, but a caller may pass it
;     `app.MapGroup("/api")` (`RouteGroupBuilder` implements it), so those routes
;     are promoted without the caller's prefix. eShop's own `MapCatalogApi(this
;     IEndpointRouteBuilder app)` is that shape, and called on the application;
;   * `MapMethods`, `MapFallback`, `MapControllerRoute` (conventional routing)
;     and a path that is not a written string literal promote nothing;
;   * an action carrying both a verb attribute and a method-level `[Route]`, or an
;     `[ActionName]`, is refused: its address is the combination.

; ── Attribute routes ────────────────────────────────────────────────────────

; `[HttpGet("users/{id}")] public IActionResult Get(int id) {…}` — the attribute
; name maps through [framework_methods] (an unmapped attribute promotes
; nothing). Only the FIRST, positional argument is the path: `Name = "x"` is not.
; `@fw.route.absolute`: a path written `"/health"` or `"~/health"` is served from
; the root and is not joined onto the class prefix.
(method_declaration
  (attribute_list
    (attribute
      name: (identifier) @fw.route.method @fw.route.rooted @fw.route.absolute
      (attribute_argument_list
        .
        (attribute_argument
          !name
          (string_literal) @fw.route.path))))
  name: (identifier) @fw.route.handler)

; `[HttpGet]` with no argument list: the action is the class prefix itself. The
; trailing anchor says the name is the attribute's last child. A bare `[Route]`
; is no registration.
((method_declaration
  (attribute_list
    (attribute
      name: (identifier) @fw.route.method @fw.route.rooted .))
  name: (identifier) @fw.route.handler)
  (#not-eq? @fw.route.method "Route"))

; ── The class route prefix ──────────────────────────────────────────────────

; `[Route("api/[controller]")] public class CatalogController` — the prefix
; governs the whole declaration, attributes and body.
((class_declaration
  (attribute_list
    (attribute
      name: (identifier) @fw.route.prefix.name @fw.route.rooted @fw.route.absolute
      (attribute_argument_list
        .
        (attribute_argument
          !name
          (string_literal) @fw.route.prefix))))
  name: (identifier) @fw.route.token.controller) @fw.route.prefix.scope
  (#eq? @fw.route.prefix.name "Route"))

; A prefix that is not a written literal (`[Route(Routes.Base)]`,
; `[Route(Template = "x")]`): every child of the first argument is marked opaque,
; so the interpreter refuses the actions under it rather than promote them at a
; partial path. For a written literal the one child is the literal itself, which
; the interpreter reads as that literal.
((class_declaration
  (attribute_list
    (attribute
      name: (identifier) @fw.route.prefix.name @fw.route.rooted @fw.route.absolute
      (attribute_argument_list
        .
        (attribute_argument
          (_) @fw.route.prefix.opaque))))
  name: (identifier) @fw.route.token.controller) @fw.route.prefix.scope
  (#eq? @fw.route.prefix.name "Route"))

; An action renamed with `[ActionName("x")]` has an `[action]` this file does not
; read: the method is a prefix scope of its own whose prefix is opaque, so the
; interpreter refuses its routes rather than promote them under the method's
; name.
((method_declaration
  (attribute_list
    (attribute
      name: (identifier) @fw.route.prefix.name
      (attribute_argument_list
        .
        (attribute_argument
          (_) @fw.route.prefix.opaque))))) @fw.route.prefix.scope
  (#eq? @fw.route.prefix.name "ActionName"))

; An action that carries a verb attribute AND a method-level `[Route("x")]` is
; addressed by the combination (`[HttpGet] [Route("{id}")]`), which this file does
; not compose: the method is a prefix scope of its own whose prefix is opaque, so
; the interpreter refuses its routes rather than promote the verb's path alone. A
; method-level `[Route("x")]` on its own is still a route (ANY). One pattern per
; arrangement of the two attributes: in separate lists, or in one, either order.
((method_declaration
  (attribute_list (attribute name: (identifier) @_verb))
  (attribute_list
    (attribute
        name: (identifier) @fw.route.prefix.name
        (attribute_argument_list
          .
          (attribute_argument
            (_) @fw.route.prefix.opaque))))) @fw.route.prefix.scope
  (#any-of? @_verb "HttpGet" "HttpPost" "HttpPut" "HttpDelete" "HttpPatch" "HttpHead")
  (#eq? @fw.route.prefix.name "Route"))

((method_declaration
  (attribute_list
    (attribute
        name: (identifier) @fw.route.prefix.name
        (attribute_argument_list
          .
          (attribute_argument
            (_) @fw.route.prefix.opaque))))
  (attribute_list (attribute name: (identifier) @_verb))) @fw.route.prefix.scope
  (#any-of? @_verb "HttpGet" "HttpPost" "HttpPut" "HttpDelete" "HttpPatch" "HttpHead")
  (#eq? @fw.route.prefix.name "Route"))

((method_declaration
  (attribute_list
    (attribute name: (identifier) @_verb)
    (attribute
        name: (identifier) @fw.route.prefix.name
        (attribute_argument_list
          .
          (attribute_argument
            (_) @fw.route.prefix.opaque))))) @fw.route.prefix.scope
  (#any-of? @_verb "HttpGet" "HttpPost" "HttpPut" "HttpDelete" "HttpPatch" "HttpHead")
  (#eq? @fw.route.prefix.name "Route"))

((method_declaration
  (attribute_list
    (attribute
        name: (identifier) @fw.route.prefix.name
        (attribute_argument_list
          .
          (attribute_argument
            (_) @fw.route.prefix.opaque)))
    (attribute name: (identifier) @_verb))) @fw.route.prefix.scope
  (#any-of? @_verb "HttpGet" "HttpPost" "HttpPut" "HttpDelete" "HttpPatch" "HttpHead")
  (#eq? @fw.route.prefix.name "Route"))

; ── Minimal APIs ────────────────────────────────────────────────────────────

; `app.MapGet("/items", GetAll)` — `@fw.group.route.receiver` is what the verb is
; called on, resolved through the group facts below. The two patterns are one
; registration (same `@fw.route.anchor`): the one that names a method-group
; handler gives the route its `RoutesTo` edge, a lambda handler is the other's.
((invocation_expression
  function: (member_access_expression
    expression: (_) @fw.group.route.receiver
    name: (identifier) @fw.route.method @fw.route.rooted)
  arguments: (argument_list
    .
    (argument !name (string_literal) @fw.route.path)
    .
    (argument !name [(identifier) (member_access_expression)] @fw.route.handler))) @fw.route.anchor
  (#any-of? @fw.route.method "MapGet" "MapPost" "MapPut" "MapDelete" "MapPatch"))

((invocation_expression
  function: (member_access_expression
    expression: (_) @fw.group.route.receiver
    name: (identifier) @fw.route.method @fw.route.rooted)
  arguments: (argument_list
    .
    (argument !name (string_literal) @fw.route.path))) @fw.route.anchor
  (#any-of? @fw.route.method "MapGet" "MapPost" "MapPut" "MapDelete" "MapPatch"))

; ── Route groups (see `resolve::framework::group`) ──────────────────────────

; A call on a member is a link of a builder chain only when it is a fluent
; builder method — one that returns the group it was called on
; (`.HasApiVersion(1.0)`, `.RequireAuthorization()`) or that starts the chain
; (`WebApplication.CreateBuilder(args).Build()`, `app.NewVersionedApi("x")`). Any
; other call (`app.MapOrders()`, `Groups.Orders(app)`) may return a group with a
; prefix this file cannot see, so it is no link and the registration on its result
; is refused. A fluent method missing from the list fails closed the same way.
((invocation_expression
  function: (member_access_expression
    expression: (_) @fw.group.receiver
    name: (identifier) @fw.group.method)) @fw.group.call
  (#any-of? @fw.group.method
    "Build" "Create" "CreateBuilder" "CreateSlimBuilder" "CreateEmptyBuilder"
    "NewVersionedApi" "HasApiVersion" "HasDeprecatedApiVersion" "AdvertiseApiVersion"
    "AdvertiseDeprecatedApiVersion" "MapToApiVersion" "IsApiVersionNeutral"
    "ReportApiVersions" "WithApiVersionSet" "RequireAuthorization" "AllowAnonymous"
    "RequireCors" "RequireHost" "RequireRateLimiting" "DisableRateLimiting"
    "DisableAntiforgery" "CacheOutput" "AddEndpointFilter" "AddEndpointFilterFactory"
    "WithTags" "WithName" "WithGroupName" "WithDisplayName" "WithDescription"
    "WithSummary" "WithMetadata" "WithOpenApi" "WithParameterValidation"
    "ExcludeFromDescription" "Produces" "ProducesProblem" "ProducesValidationProblem"
    "Accepts"))

; The static `WebApplication` is the one name that is a root wherever it is
; spelled: a chain that ends in it starts at a plain builder.
((member_access_expression
  expression: (identifier) @fw.group.root.name)
  (#eq? @fw.group.root.name "WebApplication"))

; `MapGroup` adds a prefix: whatever its first argument is, it is read — as a
; written literal when it is one, otherwise as one the pass cannot read.
; ASP.NET Core combines a group's pattern with its route's segment by segment,
; so a route written `"/"` on a group is the group itself (`@fw.group.absorbs`).
((invocation_expression
  function: (member_access_expression
    expression: (_) @fw.group.receiver
    name: (identifier) @fw.group.method @fw.group.rooted @fw.group.absorbs)
  arguments: (argument_list
    .
    (argument) @fw.group.path.opaque)) @fw.group.call
  (#eq? @fw.group.method "MapGroup"))

((invocation_expression
  function: (member_access_expression
    expression: (_) @fw.group.receiver
    name: (identifier) @fw.group.method @fw.group.rooted @fw.group.absorbs)
  arguments: (argument_list
    .
    (argument !name (string_literal) @fw.group.path))) @fw.group.call
  (#eq? @fw.group.method "MapGroup"))

; A local bound to an initializer: `var api = app.MapGroup("api/orders");`,
; visible from its declaration to the end of the block (or, in top-level
; statements, the file).
(block
  (local_declaration_statement
    (variable_declaration
      (variable_declarator
        name: (identifier) @fw.group.name
        .
        (_) @fw.group.init)))) @fw.group.scope

(compilation_unit
  (global_statement
    (local_declaration_statement
      (variable_declaration
        (variable_declarator
          name: (identifier) @fw.group.name
          .
          (_) @fw.group.init))))) @fw.group.scope

; A name assigned to after its declaration (`g = app.MapGroup("/b")`) is whatever
; it was last given: it is no longer what its declaration says.
(assignment_expression
  left: (identifier) @fw.group.assigned)

; A field or property of the enclosing type is bound across the whole type: a
; group kept in a member has a prefix this file does not state here.
(_ body: (declaration_list
  (field_declaration
    (variable_declaration
      (variable_declarator
        name: (identifier) @fw.group.name @fw.group.hoisted))))) @fw.group.scope

(_ body: (declaration_list
  (property_declaration
    name: (identifier) @fw.group.name @fw.group.hoisted))) @fw.group.scope

; A parameter of a plain endpoint-builder type is a root: no group is in front
; of it. Any other parameter may be a group the caller chose.
((_
  parameters: (parameter_list
    (parameter
      type: (_) @fw.group.type @fw.group.root
      name: (identifier) @fw.group.name))) @fw.group.scope
  (#any-of? @fw.group.type "IEndpointRouteBuilder" "WebApplication"))

((_
  parameters: (parameter_list
    (parameter
      type: (_) @fw.group.type
      name: (identifier) @fw.group.name))) @fw.group.scope
  (#not-any-of? @fw.group.type "IEndpointRouteBuilder" "WebApplication"))

; Untyped lambda parameters: `group => group.MapGet(…)`, `(a, b) => …`.
(lambda_expression
  parameters: (implicit_parameter) @fw.group.name) @fw.group.scope

(lambda_expression
  parameters: (parameter_list
    (parameter
      !type
      name: (identifier) @fw.group.name))) @fw.group.scope

; ── Controller components ───────────────────────────────────────────────────

; ASP.NET Core controller class: an `[ApiController]`-attributed type is the
; wired application building block (FR-FW-02). `@fw.component.base` exists only
; for the predicate.
((class_declaration
  (attribute_list
    (attribute
      name: (identifier) @fw.component.base))
  name: (identifier) @fw.component.name)
  (#any-of? @fw.component.base "ApiController"))
