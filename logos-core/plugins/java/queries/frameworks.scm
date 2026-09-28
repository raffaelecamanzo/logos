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
; annotated handler method to link.
;
; Captured, but NOT interpreted: a property placeholder (`value =
; "${api.base}/x"`) is a written literal, so a method path holding one is
; promoted verbatim; resolving it against the property sources is out of scope
; (FR-FW-05).
;
; A *method* path that is present but not a written literal — a constant
; reference or a concatenation (`value = "/users/{" + USER_ID + "}"`) — is
; captured as `@fw.route.path.opaque` / `@fw.route.path.named.opaque` (S-469,
; CR-151). The shared interpreter folds it when every operand is a literal or a
; `String` constant it can prove — declared in an enclosing type (S-469), or in
; another type of the same member reached by a single-type static import or a
; qualified `Type.X` (S-470) — from the facts captured below (`@fw.const.*`),
; and otherwise refuses it and counts it in `routes_not_composed`: never dropped
; silently. In
; a *mixed* list (`value = {"/a", BASE + "/b"}`, or positional
; `{"/a", BASE + "/b"}`) each element is judged on its own — `/a` is promoted and
; `BASE + "/b"` is folded or counted. The positional list form is read since
; S-469; the Kotlin query does not read it yet (`["/a", "/b"]` promotes nothing
; there), a divergence pinned in the JVM parity tests.
;
; A class-level prefix is stricter, because a prefix is *joined* rather than
; promoted as written: a prefix that is not a resolvable literal path — and does
; not fold to one from the constants below (S-469) — leaves every route under it
; at an unknown address, so those routes are refused wholesale
; (`path-not-composed`, FR-WS-05, NFR-RA-05) instead of promoted at a partial
; path that would falsely claim a provider. `@fw.route.prefix.opaque` below is
; how this file says "a prefix is here and it is not a literal".

; Spring request-mapping annotation on a handler method, positional form:
; `@GetMapping("/users") public List<User> list() {…}` — the annotation name
; maps through [framework_methods] (an unmapped annotation promotes nothing).
(method_declaration
  (modifiers
    (annotation
      name: (identifier) @fw.route.method
      arguments: (annotation_argument_list
        [
          (string_literal) @fw.route.path
          (element_value_array_initializer (string_literal) @fw.route.path)
        ])) @fw.route.anchor)
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

; The same two positions holding an **expression** instead of a literal
; (S-469, CR-151): `@GetMapping(BASE)`, `@GetMapping(value = "/u/{" + ID + "}")`,
; `path = {"/a", BASE + "/b"}`. Captured through the `(expression)` supertype
; for the reason the prefix section gives — every non-literal form at once, so
; an unforeseen spelling is still captured and therefore still refused and
; counted rather than lost. The supertype also matches a `string_literal`; the
; interpreter ignores an opaque path whose bytes are exactly a captured literal
; path's, so the literal patterns above keep owning every written literal.
(method_declaration
  (modifiers
    (annotation
      name: (identifier) @fw.route.method
      arguments: (annotation_argument_list
        [
          (expression) @fw.route.path.opaque
          (element_value_array_initializer (expression) @fw.route.path.opaque)
        ])) @fw.route.anchor)
  name: (identifier) @fw.route.handler)

((method_declaration
  (modifiers
    (annotation
      name: (identifier) @fw.route.method
      arguments: (annotation_argument_list
        (element_value_pair
          key: (identifier) @fw.route.key
          value: [
            (expression) @fw.route.path.named.opaque
            (element_value_array_initializer
              (expression) @fw.route.path.named.opaque)
          ]))) @fw.route.anchor)
  name: (identifier) @fw.route.handler)
  (#any-of? @fw.route.key "value" "path"))

; A mapping annotation carrying NO path at all — `@GetMapping` on a handler
; whose full path is its declaring type's prefix (FR-FW-05, S-329). The
; **marker** form only: `@GetMapping` parses to `marker_annotation`, a node kind
; neither path-bearing pattern above can match, so this pattern never competes
; for a registration site with them — the anchor-pairing invariant holds
; without the interpreter having to rank anything. (`@GetMapping()`, with empty
; parens, parses as an argument-bearing `annotation` and is deliberately left
; uncaptured as a degenerate shape. It *could* be matched — a text predicate on
; the captured argument list, `(#eq? @args "()")`, constrains it precisely — but
; a structural pattern alone cannot, and nobody writes the form.)
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
;   @fw.route.prefix.opaque — a node in the prefix position that is not a
;                             readable literal. It disqualifies every prefix
;                             literal its bytes overlap without being identical
;                             to, so it can refuse one element of a list, or a
;                             fragment inside a literal, rather than the whole
;                             scope;
;   @fw.route.prefix.name / @fw.route.prefix.key — predicate-only.
;
; A type may declare several literal prefixes (`value = {"/a", "/b"}`); the
; interpreter fans each route out over them. In a mixed list
; (`value = {"/a", BASE}`) the opaque capture overlaps only `BASE`, so `/a`
; still composes — the same "promote what is established, drop the rest" rule a
; mixed method-path list already follows. An explicitly empty `value = {}`
; captures nothing at all and therefore declares no prefix, exactly as Spring
; reads it.

; Every type declaration is a prefix **boundary**, whether or not it declares
; one. A nested type is its own controller bean in Spring and does NOT inherit
; its enclosing type's `@RequestMapping` prefix, so without this pattern a
; handler in an unannotated inner class of a prefixed outer class would be
; promoted at a prefix it is not actually served under. A boundary that
; declares no prefix reads as "unprefixed" — never as a refusal — because it
; shares its range with, and is combined into, the prefix patterns below when
; the same declaration matched both.
;
; "A type declaration" is expressed as a wildcard node whose `body:` is one of
; the three type-body kinds — `class_body` (class AND record), `interface_body`,
; `enum_body`. That covers all four kinds Spring can put a controller on in one
; pattern, and it excludes `method_declaration` (whose body is a `block`), which
; is the shape that matters: a method-level `@RequestMapping` must never be read
; as a prefix governing its own route. The grammar's `declaration` supertype
; looks like the obvious spelling and is NOT usable here — the query engine
; matches `method_declaration` through it, which self-composes every named
; mapping into `/v1/x/v1/x`.
(_ body: [(class_body) (interface_body) (enum_body)]) @fw.route.prefix.scope

; The prefix itself, one pattern per argument form over that same wildcard —
; a record or an enum can carry `@RequestMapping` as readily as a class, and
; omitting them would not "lose a prefix safely": the boundary above would still
; match, so the type would read as unprefixed and its handlers would be promoted
; at their bare method paths, a wrong address rather than an absent one.
;
; The `.opaque` half is what makes a capture gap fail CLOSED. It captures the
; `(expression)` supertype — every non-literal argument form at once, with no
; enumeration to fall behind — which also matches a `string_literal` in the
; same position; the interpreter ignores an opaque range identical to a
; captured literal, so the literal patterns still win wherever the argument
; reads. `element_value_pair` is not an `expression`, so the positional
; catch-all can never mistake `method = RequestMethod.GET` for a path.

; Positional prefix: `@RequestMapping("/v1")`, `@RequestMapping({"/v1","/v2"})`,
; `@RequestMapping(BASE)`, `@RequestMapping(BASE + "/v1")`.
((_ (modifiers
      (annotation
        name: (identifier) @fw.route.prefix.name
        arguments: (annotation_argument_list
          [
            (string_literal) @fw.route.prefix
            (element_value_array_initializer (string_literal) @fw.route.prefix)
          ])))
    body: [(class_body) (interface_body) (enum_body)]) @fw.route.prefix.scope
  (#eq? @fw.route.prefix.name "RequestMapping"))

((_ (modifiers
      (annotation
        name: (identifier) @fw.route.prefix.name
        arguments: (annotation_argument_list
          [
            (expression) @fw.route.prefix.opaque
            (element_value_array_initializer (expression) @fw.route.prefix.opaque)
          ])))
    body: [(class_body) (interface_body) (enum_body)]) @fw.route.prefix.scope
  (#eq? @fw.route.prefix.name "RequestMapping"))

; Named prefix: `@RequestMapping(value = "/v1", produces = ...)` and its list
; form. `value`/`path` are Spring aliases, so both keys count — and the key
; predicate is what stops a sibling string argument (`produces = "text/plain"`)
; from being read as a prefix.
((_ (modifiers
      (annotation
        name: (identifier) @fw.route.prefix.name
        arguments: (annotation_argument_list
          (element_value_pair
            key: (identifier) @fw.route.prefix.key
            value: [
              (string_literal) @fw.route.prefix
              (element_value_array_initializer (string_literal) @fw.route.prefix)
            ]))))
    body: [(class_body) (interface_body) (enum_body)]) @fw.route.prefix.scope
  (#eq? @fw.route.prefix.name "RequestMapping")
  (#any-of? @fw.route.prefix.key "value" "path"))

((_ (modifiers
      (annotation
        name: (identifier) @fw.route.prefix.name
        arguments: (annotation_argument_list
          (element_value_pair
            key: (identifier) @fw.route.prefix.key
            value: [
              (expression) @fw.route.prefix.opaque
              (element_value_array_initializer (expression) @fw.route.prefix.opaque)
            ]))))
    body: [(class_body) (interface_body) (enum_body)]) @fw.route.prefix.scope
  (#eq? @fw.route.prefix.name "RequestMapping")
  (#any-of? @fw.route.prefix.key "value" "path"))

; ── `String` constants a path may fold from (S-469, CR-151, FR-FW-05) ──────────
;
; The fold is exact or nothing (NFR-RA-05): a name folds only when the
; interpreter can prove which declaration Java binds it to. These captures give
; it the facts that proof needs, and nothing here decides anything:
;
;   @fw.const.scope        — a type **body**: the region in which the type's own
;                            members are in scope by simple name. The body, not
;                            the declaration, because Java resolves a type's own
;                            annotations *outside* it — `@RequestMapping(BASE)`
;                            on `class C` cannot see `C.BASE` by its simple name;
;   @fw.const.scope.decl   — the whole type declaration, annotations included:
;                            where the qualified `C.BASE` names this type;
;   @fw.const.scope.name   — the type's simple name, for that qualified form;
;   @fw.const.scope.opaque — a sign the scope can declare names this file does
;                            not model: a supertype (whose inherited fields a
;                            simple name may bind to), enum constants, or an
;                            anonymous class. A lookup reaching such a scope
;                            without finding its own declaration refuses;
;   @fw.const.field        — the name of **every** field declared in a body,
;                            whatever its type or modifiers: a non-constant field
;                            still shadows a same-named constant further out, so
;                            the shadow has to be known to refuse on it;
;   @fw.const.name / @fw.const.value — a compile-time `String` constant (a
;                            `static final String` field, or any interface
;                            `String` field, which Java makes implicitly
;                            `public static final`) and its initializer;
;   @fw.const.type         — predicate-only;
;   @fw.const.import       — an import declaration's path (S-470): where a name
;                            the file does not declare may still come from. A
;                            single-type `import static a.b.C.X;` makes `X` the
;                            constant `X` of type `a.b.C`; a single-type
;                            `import a.b.C;` makes the qualified `C.X` name it.
;                            The interpreter finds `C`'s file only inside the
;                            member being indexed, through the package-shaped
;                            module key (S-465), never by splitting a path;
;   @fw.const.import.static / @fw.const.import.asterisk — markers present in
;                            the same match for a static import and for a
;                            wildcard. A wildcard never supplies a constant: it
;                            only tells the interpreter that a name might come
;                            from somewhere it cannot prove, so it refuses.
;
; A query that captures none of these opts out of folding altogether, which is
; how the Kotlin query keeps its `const val` and string templates out of scope
; (CR-151 §3.3) without a line of language-specific interpreter code.

; Every **named type declaration**, spelled out rather than matched as a wildcard
; over `name:`/`body:`: the wildcard would miss an `@interface` (whose body is an
; `annotation_type_body`), whose `String` constants would then be filed under the
; enclosing class as if it declared them — and it would give an enum constant's
; body a *name*, which is not a type a qualified reference can name.
[
  (class_declaration
    name: (identifier) @fw.const.scope.name body: (class_body) @fw.const.scope)
  (record_declaration
    name: (identifier) @fw.const.scope.name body: (class_body) @fw.const.scope)
  (interface_declaration
    name: (identifier) @fw.const.scope.name body: (interface_body) @fw.const.scope)
  (enum_declaration
    name: (identifier) @fw.const.scope.name body: (enum_body) @fw.const.scope)
  (annotation_type_declaration
    name: (identifier) @fw.const.scope.name body: (annotation_type_body) @fw.const.scope)
] @fw.const.scope.decl

(class_declaration (superclass) @fw.const.scope.opaque body: (class_body) @fw.const.scope)
(class_declaration (super_interfaces) @fw.const.scope.opaque body: (class_body) @fw.const.scope)
(record_declaration (super_interfaces) @fw.const.scope.opaque body: (class_body) @fw.const.scope)
(interface_declaration (extends_interfaces) @fw.const.scope.opaque body: (interface_body) @fw.const.scope)
(enum_declaration body: (enum_body) @fw.const.scope @fw.const.scope.opaque)
(object_creation_expression (class_body) @fw.const.scope @fw.const.scope.opaque)
(enum_constant body: (class_body) @fw.const.scope @fw.const.scope.opaque)

(field_declaration
  declarator: (variable_declarator name: (identifier) @fw.const.field))
(constant_declaration
  declarator: (variable_declarator name: (identifier) @fw.const.field))

; `static` and `final` in either order — Java admits both, and a query child
; list matches in order.
((field_declaration
  [
    (modifiers "static" "final")
    (modifiers "final" "static")
  ]
  type: (_) @fw.const.type
  declarator: (variable_declarator
    name: (identifier) @fw.const.name
    value: (_) @fw.const.value))
  (#any-of? @fw.const.type "String" "java.lang.String"))

((constant_declaration
  type: (_) @fw.const.type
  declarator: (variable_declarator
    name: (identifier) @fw.const.name
    value: (_) @fw.const.value))
  (#any-of? @fw.const.type "String" "java.lang.String"))

; Every import declaration, with its markers as optional children of one pattern
; — the shape `references.scm` uses, for the reason it gives: a comment is a
; named node too, so a second pattern kept apart by a last-child anchor would
; miss `import a.b.C /* why */;`.
(import_declaration
  "static"? @fw.const.import.static
  (scoped_identifier) @fw.const.import
  (asterisk)? @fw.const.import.asterisk)

; Spring stereotype class: the wired application building block (FR-FW-02).
; `@fw.component.base` exists only for the predicate.
((class_declaration
  (modifiers
    (marker_annotation
      name: (identifier) @fw.component.base))
  name: (identifier) @fw.component.name)
  (#any-of? @fw.component.base
    "Component" "Service" "Repository" "Controller" "RestController" "Configuration"))
