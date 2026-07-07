; Rust framework-extraction query (S-012, capability = "frameworks").
;
; Captures the *anchors* of framework usage — the AST sites where an Axum or
; Actix-web route or shared-state component may be declared. Anchors are
; deliberately broad and carry no framework judgment of their own: the
; framework pass (resolve::framework) walks each anchor's subtree in code and
; applies the precise Axum/Actix shape rules there, because a tree-sitter
; query cannot express recursive method-router chains (`get(a).post(b)`) or
; relate an attribute to its following item. The pass only ever runs these
; anchors against files whose reference ledger names a framework crate
; (FR-FW-04: a plain library is never parsed at all).
;
; The capture name after the `@` carries the anchor shape:
;   @fw.route — a `.route(…)` registration candidate: any method call whose
;               first argument is a string literal. The pass checks the method
;               name is `route` and pattern-matches the second argument as an
;               Axum method-router chain or an Actix `web::method().to(h)`.
;   @fw.attr  — an attribute with a parenthesised string argument: the Actix
;               method-macro candidate (`#[get("/p")]`). The pass checks the
;               attribute name against the HTTP-method set and binds the
;               following `function_item` as the handler.
;   @fw.param — a generic-typed function parameter: the shared-state component
;               candidate. The pass checks the generic head is `State`/`Data`
;               (axum::extract::State / actix_web::web::Data) and promotes the
;               first type argument when it binds to an indexed type.
;
; Like every capability query, this file is droppable-on-disk: a copy at
; `.logos/plugins/rust/queries/frameworks.scm` shadows it without a rebuild
; (FR-PL-04, FR-PL-05).
;
; Deliberately NOT captured in v1 (documented limitations, S-012):
;   - Actix `web::resource("/p").route(…)` (path on the receiver chain);
;   - Actix `#[route("/p", method = "GET")]` (generic route attribute);
;   - routes registered through macros or built dynamically.

(call_expression
  function: (field_expression
    field: (field_identifier))
  arguments: (arguments
    .
    [(string_literal) (raw_string_literal)])) @fw.route

(attribute_item
  (attribute
    (identifier)
    arguments: (token_tree
      [(string_literal) (raw_string_literal)]))) @fw.attr

(parameter
  type: (generic_type)) @fw.param
