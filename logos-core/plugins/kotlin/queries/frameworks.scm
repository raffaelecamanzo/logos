; Kotlin framework-extraction query (S-055, S-330, capability = "frameworks") —
; the ratified JVM set: Spring MVC/Boot (FR-FW-03, FR-FW-05),
; annotation-compatible with the Java detector: the same `@GetMapping`
; /stereotype idiom, the same `[framework_methods]` mapping and the same
; `org::springframework` candidacy gate.
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
; surface is the capture names below, which is the claim S-330 verifies.
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
; Deliberately NOT captured: functional `RouterFunction` routing, whose builder
; chain names no annotated handler method to link; a *method* path that is not
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
; no pattern in this file (or any query) can see it. The same defect swallows
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
(function_declaration
  (modifiers
    (annotation
      (constructor_invocation
        (user_type (identifier) @fw.route.method)
        (value_arguments
          (value_argument
            .
            (string_literal) @fw.route.path))) @fw.route.anchor))
  name: (identifier) @fw.route.handler)

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
        (user_type (identifier) @fw.route.method)
        (value_arguments
          (value_argument
            .
            (identifier) @fw.route.key
            [
              (string_literal) @fw.route.path.named
              (collection_literal (string_literal) @fw.route.path.named)
            ]))) @fw.route.anchor))
  name: (identifier) @fw.route.handler)
  (#any-of? @fw.route.key "value" "path" "`value`" "`path`"))

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
(function_declaration
  (modifiers
    (annotation
      (user_type (identifier) @fw.route.method) @fw.route.anchor))
  name: (identifier) @fw.route.handler)

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
          (user_type (identifier) @fw.route.prefix.name)
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
          (user_type (identifier) @fw.route.prefix.name)
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
          (user_type (identifier) @fw.route.prefix.name)
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
          (user_type (identifier) @fw.route.prefix.name)
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
