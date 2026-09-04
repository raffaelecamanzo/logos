; TSX framework-extraction query (S-015, capability = "frameworks") —
; the ratified set: Express + Next.js (FR-FW-03).
;
; Declarative capture contract (resolve::framework::generic_match): see the
; Python query's header for the capture vocabulary. Droppable on disk at
; `.logos/plugins/tsx/queries/frameworks.scm`.
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
;
; The receiver is scoped to an Express router (CR-110). Unscoped, this pattern
; matched **any** `<expr>.get("string")` call — an Angular `formGroup.get("year")`
; control lookup and a `state.get("active")` property read in a vendored bundle
; both promoted a route — and the repo-scoped `framework_detectors` gate does not
; bound it: one React dependency admits extraction across the whole repo,
; vendored assets included.
;
; What this can and cannot assert: a tree-sitter pattern is local, so it cannot
; resolve `app` back to its `express()`/`express.Router()` binding. The
; constraint is therefore on the receiver's conventional *name* — `app`,
; `router`, `server`, or a camelCase `…Router`/`…App`/`…Server`, in either case,
; optionally member-qualified (`this.app`, `self.router`). The handler-bearing
; pattern above is deliberately left unscoped, so an unconventionally-named
; receiver still promotes every registration that names its handler; and the
; shared pass's path guard catches whatever the name rule admits.
((call_expression
  function: (member_expression
    object: [(identifier) (member_expression) (this)] @fw.route.receiver
    property: (property_identifier) @fw.route.method)
  arguments: (arguments
    .
    (string) @fw.route.path))
  (#match? @fw.route.receiver "(^|\\.)([Aa]pp|[Rr]outer|[Ss]erver|[A-Za-z_$][A-Za-z0-9_$]*(Router|App|Server))$"))

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
