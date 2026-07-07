; Python framework-extraction query (S-015, capability = "frameworks") — the
; ratified set: FastAPI + Django (FR-FW-03).
;
; Declarative capture contract (resolve::framework::generic_match): a pattern
; captures the registration parts directly —
;   @fw.route.path    — the URL string literal (unquoted by the pass);
;   @fw.route.method  — the node whose text maps through [framework_methods]
;                       (unmapped text drops the match, FR-FW-04 best-effort);
;   @fw.route.handler — optional; a plain (possibly dotted) handler name the
;                       binder must prove (NFR-RA-05, never fabricate);
;   @fw.component.name — a component declaration's name;
;   @fw.component.base — predicate-only helper, not consumed by the pass.
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
