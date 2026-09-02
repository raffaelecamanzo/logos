; Python framework-extraction query (S-015, capability = "frameworks") — the
; ratified set: FastAPI + Django (FR-FW-03).
;
; Declarative capture contract (resolve::framework::generic_match): a pattern
; captures the registration parts directly —
;   @fw.route.path    — the URL string literal (unquoted by the pass). The
;                       pass collects every such capture in a match and
;                       registers one route per capture, so a `+`/`*`
;                       quantifier works. Note a list-valued argument does NOT
;                       need one: an unanchored child pattern already yields
;                       one *match* per element (`value = {"/a", "/b"}` → two
;                       matches, one path each);
;   @fw.route.path.named — the same, for a path written as a *named* argument
;                       (Java/Kotlin `value =`/`path =`). Outranks a plain
;                       @fw.route.path at the same @fw.route.anchor (S-328);
;   @fw.route.anchor  — optional; the registration site (the annotation, the
;                       call) that paths are ranked within. Only a query whose
;                       patterns can both match one site needs it — but then it
;                       needs it on EVERY such pattern: a path with no anchor
;                       competes with nothing and always survives, so anchoring
;                       the named pattern while leaving a pre-existing
;                       positional one unanchored silently promotes both;
;   @fw.route.method  — the node whose text maps through [framework_methods]
;                       (unmapped text drops the match, FR-FW-04 best-effort);
;   @fw.route.handler — optional; a plain (possibly dotted) handler name the
;                       binder must prove (NFR-RA-05, never fabricate);
;   @fw.route.prefix  — a class-/interface-level path prefix every route
;                       declared inside its scope is joined onto (S-329,
;                       FR-FW-05). Collected as a list like @fw.route.path, so
;                       a type declaring several bases fans each route out;
;   @fw.route.prefix.scope — the declaration whose byte range the prefix
;                       governs. REQUIRED alongside @fw.route.prefix: the pass
;                       matches a route to a prefix by containment, and the
;                       *innermost* containing scope wins, so a prefixed nested
;                       type takes its own prefix. Must span the whole
;                       declaration, annotation and body — a scope that stops
;                       short of the body contains no handler and silently
;                       composes nothing;
;   @fw.route.prefix.opaque — a prefix argument that is NOT a written literal
;                       (a constant, a concatenation). Its mere PRESENCE marks
;                       the scope non-composable: with no literal alongside it,
;                       routes inside are refused rather than promoted at a
;                       partial path (`path-not-composed`, FR-WS-05,
;                       NFR-RA-05). Capture it only where the argument really
;                       is in the path position — marking a `method =` argument
;                       opaque would refuse a whole controller. A prefix whose
;                       *text* names an unresolved reference — `${…}`, `#{…}`,
;                       or a Kotlin string template `$name` — is refused by the
;                       interpreter without any capture at all (S-330), which is
;                       what covers the fragments a grammar models no node for;
;   @fw.component.name — a component declaration's name;
;   @fw.component.base — predicate-only helper, not consumed by the pass.
;
; Any other `@fw.route.*` / `@fw.component.*` capture is predicate-only too —
; the Java query's `@fw.route.key` filters argument names with `#any-of?`, and
; its `@fw.route.prefix.name`/`@fw.route.prefix.key` pin which class-level
; annotation is a prefix.
;
; Composition itself — separator normalisation across all four slash cases, the
; prefix-as-whole-path fallback for an annotation that named none, the
; non-literal refusal — lives once in `compose_prefixes` and is NOT expressible
; in a query file (a pattern captures, it cannot concatenate). A prefixing
; dialect therefore inherits every one of those rules by naming the three
; captures above and writing no code (S-330).
;
; A registration that captures @fw.route.method and @fw.route.handler but NO
; path is not zero routes: it is a *pathless* candidate whose full path is its
; prefix. With no prefix in scope it promotes nothing and reports nothing —
; absence of a prefix is never a composition failure (BR-46).
;
; Droppable on disk at `.logos/plugins/python/queries/frameworks.scm`.
;
; Deliberately NOT captured in v1: FastAPI `app.add_api_route(...)`, Django
; class-based views (`views.X.as_view()` handlers stay unproven), router
; `include(...)` indirection.

; FastAPI method decorator: `@app.get("/users")` on a `def` — the decorated
; function is the handler.
(decorated_definition
  (decorator
    (call
      function: (attribute
        attribute: (identifier) @fw.route.method)
      arguments: (argument_list
        .
        (string) @fw.route.path)))
  definition: (function_definition
    name: (identifier) @fw.route.handler))

; Django URLconf registration: `path("users/", views.list_users)` /
; `re_path(...)`. The function name maps to ANY via [framework_methods];
; any other single-identifier call with a leading string is dropped there.
(call
  function: (identifier) @fw.route.method
  arguments: (argument_list
    .
    (string) @fw.route.path
    .
    [(identifier) (attribute)] @fw.route.handler))

; Django model: a class whose base is `models.Model` (or a `…Model` subclass
; path) is the wired application building block (FR-FW-02).
((class_definition
  name: (identifier) @fw.component.name
  superclasses: (argument_list
    [(identifier) (attribute)] @fw.component.base))
  (#match? @fw.component.base "Model$"))
