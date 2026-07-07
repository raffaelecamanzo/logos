; TypeScript framework-extraction query (S-015, capability = "frameworks") —
; the ratified set: Express + Next.js (FR-FW-03).
;
; Declarative capture contract (resolve::framework::generic_match): see the
; Python query's header for the capture vocabulary. Droppable on disk at
; `.logos/plugins/typescript/queries/frameworks.scm`.
;
; Deliberately NOT captured in v1: `router.route("/p").get(h)` chains,
; Next.js file-system routes (the path lives in the file name, not the AST),
; App-Router `export function GET()` handlers (no path in the AST).

; Express registration with a provable handler: `app.get("/users", listUsers)`.
(call_expression
  function: (member_expression
    property: (property_identifier) @fw.route.method)
  arguments: (arguments
    .
    (string) @fw.route.path
    .
    [(identifier) (member_expression)] @fw.route.handler))

; …and the handler-less form (inline closures, middleware chains): the route
; node is still promoted, with no fabricated edge (NFR-RA-05). Overlap with
; the pattern above collapses in the pass (dedup prefers the proven handler).
(call_expression
  function: (member_expression
    property: (property_identifier) @fw.route.method)
  arguments: (arguments
    .
    (string) @fw.route.path))

; Next.js/React component: an exported PascalCase function declaration —
; the UI building block (FR-FW-02, UAT-FW-02).
((export_statement
  declaration: (function_declaration
    name: (identifier) @fw.component.name))
  (#match? @fw.component.name "^[A-Z]"))

((export_statement
  declaration: (lexical_declaration
    (variable_declarator
      name: (identifier) @fw.component.name
      value: (arrow_function))))
  (#match? @fw.component.name "^[A-Z]"))
