; PHP framework-extraction query (S-060, capability = "frameworks") — the
; ratified set: Laravel (FR-FW-03).
;
; Declarative capture contract (resolve::framework::generic_match): see the
; Python query's header for the capture vocabulary. Droppable on disk at
; `.logos/plugins/php/queries/frameworks.scm`.
;
; The framework pass only runs on files whose ledger names the `Illuminate`
; canonical prefix (FR-FW-04 candidacy gate, set in plugin.toml
; `framework_detectors`), so a `Route::` static call or a `Model` base in a
; non-Laravel file never promotes anything.
;
; Deliberately NOT captured in v1: route handlers (Laravel passes them as
; `[Controller::class, 'method']` arrays or `'Controller@method'` strings, not
; plain provable names — the route node is promoted with no fabricated
; RoutesTo edge, NFR-RA-05), route groups / prefixes (`Route::prefix(...)`),
; resource/closure routes, attribute-based controller routes.

; Laravel route facade: `Route::get('/users', …)` — the verb maps through
; [framework_methods] (an unmapped verb promotes nothing); the scope must be the
; `Route` facade. The first argument is the URL path string (unquoted by the pass).
((scoped_call_expression
  scope: (name) @_scope
  name: (name) @fw.route.method
  arguments: (arguments
    .
    (argument (string) @fw.route.path)))
  (#eq? @_scope "Route"))

; Laravel building block (FR-FW-02): an Eloquent model (`class X extends Model`)
; or a controller (`extends Controller`) — the base name (or its last namespace
; segment) ends in `Model`/`Controller`. `@fw.component.base` exists only for the
; predicate.
((class_declaration
  name: (name) @fw.component.name
  (base_clause [(name) (qualified_name)] @fw.component.base))
  (#match? @fw.component.base "(Model|Controller)$"))
