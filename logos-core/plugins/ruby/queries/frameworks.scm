; Ruby framework-extraction query (S-059, capability = "frameworks") — the
; ratified set: Rails (FR-FW-03).
;
; Declarative capture contract (resolve::framework::generic_match): a pattern
; captures the registration parts directly —
;   @fw.route.path    — the URL string literal (unquoted by the pass);
;   @fw.route.method  — the node whose text maps through [framework_methods]
;                       (unmapped text drops the match, FR-FW-04 best-effort);
;   @fw.component.name — a component declaration's name;
;   @fw.component.base — predicate-only helper, not consumed by the pass.
;
; Droppable on disk at `.logos/plugins/ruby/queries/frameworks.scm`.
;
; Deliberately NOT captured in v1: the RESTful `resources`/`resource` macros
; (each expands to up to seven routes — statically expanding them would
; fabricate, NFR-RA-05), `root`, and `match … via:`. The controller#action
; handler string (`"users#index"`) is intentionally not captured as a handler:
; it is not a resolvable symbol name, so binding it would fabricate a RoutesTo
; edge (NFR-RA-05). Route nodes are still created; they simply carry no edge.

; Rails route DSL inside `routes.draw do … end`: `get "/users", to: "users#index"`.
; The verb identifier maps through [framework_methods]; the first string argument
; is the path.
(call
  !receiver
  method: (identifier) @fw.route.method
  arguments: (argument_list
    .
    (string) @fw.route.path))

; A Rails controller or model: a class whose superclass names a Rails base
; (`ApplicationController`/`…Controller`, `ApplicationRecord`/`…Record`,
; `ActiveRecord::Base`) is the wired application building block (FR-FW-02).
((class
  name: (constant) @fw.component.name
  superclass: (superclass
    [(constant) (scope_resolution)] @fw.component.base))
  (#match? @fw.component.base "(Controller|Record|Base)$"))
