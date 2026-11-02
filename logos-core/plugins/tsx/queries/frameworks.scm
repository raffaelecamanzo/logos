; TSX framework-extraction query (S-015, capability = "frameworks") —
; the ratified set: Express + Next.js + NestJS (FR-FW-03, S-533).
;
; Declarative capture contract (resolve::framework::generic_match): see the
; Python query's header for the capture vocabulary. Droppable on disk at
; `.logos/plugins/tsx/queries/frameworks.scm`.
;
; Deliberately NOT captured in v1: `router.route("/p").get(h)` chains,
; Next.js file-system routes (the path lives in the file name, not the AST),
; App-Router `export function GET()` handlers (no path in the AST). An Express
; router returned from a function or passed to one is not followed to its mount
; (S-822): its registrations are refused and counted like a router mounted
; nowhere. A registration on an `app` or `router` parameter keeps its written path.

; Express registration with a provable handler: `app.get("/users", listUsers)`.
;
; Express's verbs are lower-case. `[framework_methods]` also maps NestJS's
; capitalised decorator names (`Get`, `Delete`, … S-533), and this pattern has no
; receiver to say it is not `client.Get("/users", cb)` — so a capitalised method
; name is not an Express registration (S-533).
((call_expression
  function: (member_expression
    object: (_) @fw.mount.route.receiver
    property: (property_identifier) @fw.route.method @fw.mount.route.verb)
  arguments: (arguments
    .
    (string) @fw.route.path
    .
    [(identifier) (member_expression)] @fw.route.handler))
  (#not-match? @fw.route.method "^[A-Z]"))

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
;
; A **bare** `this` receiver is deliberately not admitted, which is why the
; alternation names no `(this)`: `this.get("x")` in a class extending Router is
; a real registration, but it is textually identical to the property getter
; every other class writes, and admitting it would reopen the whole
; false-positive class this pattern exists to close. `this.app.get(…)` and
; `this.router.get(…)` — the qualified spellings — are admitted as
; member expressions.
((call_expression
  function: (member_expression
    object: [(identifier) (member_expression)] @fw.route.receiver @fw.mount.route.receiver
    property: (property_identifier) @fw.route.method @fw.mount.route.verb)
  arguments: (arguments
    .
    (string) @fw.route.path))
  (#match? @fw.route.receiver "(^|\\.)([Aa]pp|[Rr]outer|[Ss]erver|[A-Za-z_$][A-Za-z0-9_$]*(Router|App|Server))$")
  (#not-match? @fw.route.method "^[A-Z]"))

; An Express path written as a template literal: `` app.get(`${base}/ping`, h) ``
; (S-541 for JavaScript; S-822 moved it here, so TypeScript reads it too). The two
; Express patterns above take a plain `(string)` path; these are the same two
; shapes over a `(template_string)`. Both carry the handler-less pattern's
; receiver rule, the handler-bearing one included: a template is how a client
; call spells an interpolated URL (`axios.get(`/api/items/${id}`, config)`,
; `https.get(`${base}/health`, onResponse)`), and unscoped, that call would be a
; route and its callback a fabricated handler (NFR-RA-05).
;
; Which templates are paths is the shared pass's rule, not this file's
; (`resolve::framework::is_a_url_path`, CR-110): the path is read verbatim, and
; kept when it carries a `/` or opens with a `${…}` reference. A template whose
; static tail starts with `/` is therefore a route, and its leading
; interpolation — a prefix this pass does not resolve — stays in the route's
; path (`GET ${base}/ping`) as a Spring `${api.base}` placeholder does (S-328);
; the shared route-template normalizer refuses to bind it.

((call_expression
  function: (member_expression
    object: [(identifier) (member_expression)] @fw.route.receiver @fw.mount.route.receiver
    property: (property_identifier) @fw.route.method @fw.mount.route.verb)
  arguments: (arguments
    .
    (template_string) @fw.route.path
    .
    [(identifier) (member_expression)] @fw.route.handler))
  (#match? @fw.route.receiver "(^|\\.)([Aa]pp|[Rr]outer|[Ss]erver|[A-Za-z_$][A-Za-z0-9_$]*(Router|App|Server))$")
  (#not-match? @fw.route.method "^[A-Z]"))

((call_expression
  function: (member_expression
    object: [(identifier) (member_expression)] @fw.route.receiver @fw.mount.route.receiver
    property: (property_identifier) @fw.route.method @fw.mount.route.verb)
  arguments: (arguments
    .
    (template_string) @fw.route.path))
  (#match? @fw.route.receiver "(^|\\.)([Aa]pp|[Rr]outer|[Ss]erver|[A-Za-z_$][A-Za-z0-9_$]*(Router|App|Server))$")
  (#not-match? @fw.route.method "^[A-Z]"))

; ── Express routers and their mounts (S-822, CR-255, FR-FW-03, FR-FW-05) ─────
;
; `router.get('/items', list)` on `const router = express.Router()` is served
; under the path the router is mounted at — `app.use('/api', router)`, usually in
; another file. This file CAPTURES the facts and never joins them
; (`resolve::framework::mount`): the pass holds a registration on a router back,
; and a run-level join over every candidate file gives it each mount's path. A
; router no candidate file mounts, or mounted at a path that is not a written
; literal, is refused and counted; a `use` that mounts a router is not a route.
;
; Each `@fw.route.*` pattern above carries `@fw.mount.route.receiver` on the
; registration's receiver. The facts below are plain syntax:
;
;   @fw.mount.framework — a string naming the framework module, `'express'`.
;   @fw.mount.absorbs — the framework serves a router's path of exactly `/` at the
;                      mount path itself, as `@fw.group.absorbs` says for a group.
;   @fw.mount.router + @fw.mount.scope — a name bound to a router over a range,
;                      with its maker: `express.Router()` (@fw.mount.router.module,
;                      which must be imported from the framework module), `Router()`
;                      (@fw.mount.router.ctor, imported from it by name), or
;                      `require('express').Router()` (@fw.mount.router.from).
;   @fw.mount.shadow — any other binding (a parameter, a declaration of another
;                      value, a loop or `catch` variable), which hides an outer
;                      router of its name.
;   @fw.mount.import.* — a name bound to a module: `.name`, `.module` (the
;                      specifier), `.export` / `.named` (a named export, aliased or
;                      not) or `.namespace`; neither marker is the default export.
;   @fw.mount.export.* — a local the module exports: `.local`, `.name` / `.same`
;                      (a named export); neither is the default export.
;   @fw.mount.route.verb / @fw.mount.verb — a registration's method name and a
;                      mount call's `use`: the route a mount call made is the one
;                      whose verb is the call's, exactly (a chained `a.use(x).use(…)`
;                      holds two calls in one callee).
;   @fw.mount.callee — a mount call `x.use(…)`: `@fw.mount.receiver`, each argument
;                      that may be the router (`@fw.mount.arg`, or the specifier of
;                      an inline `require`, `@fw.mount.arg.module`), and the first
;                      argument (`@fw.mount.first`) — a path when it is a string
;                      (`@fw.mount.path`), none when it is middleware
;                      (`@fw.mount.middleware`: a call or a function), otherwise a
;                      value that may hold a path, which refuses.

; Express serves a router's `/` at its mount path (`router.get('/')` under `/api`
; is `GET /api`): its routers absorb (`@fw.mount.absorbs`).
((string) @fw.mount.framework @fw.mount.absorbs
  (#match? @fw.mount.framework "^[\"']express[\"']$"))

; `const router = express.Router()`, `var router = Router()`,
; `const router = require('express').Router()` — the binding is visible after its
; name, over the block (or program) that declares it.
((_
  [
    (lexical_declaration
      (variable_declarator
        name: (identifier) @fw.mount.router
        value: (call_expression
          function: [
            (member_expression
              object: [
                (identifier) @fw.mount.router.module
                (call_expression
                  function: (identifier) @_require
                  arguments: (arguments . (string) @fw.mount.router.from .))
              ]
              property: (property_identifier) @_router)
            (identifier) @fw.mount.router.ctor
          ])))
    (variable_declaration
      (variable_declarator
        name: (identifier) @fw.mount.router
        value: (call_expression
          function: [
            (member_expression
              object: [
                (identifier) @fw.mount.router.module
                (call_expression
                  function: (identifier) @_require
                  arguments: (arguments . (string) @fw.mount.router.from .))
              ]
              property: (property_identifier) @_router)
            (identifier) @fw.mount.router.ctor
          ])))
  ]) @fw.mount.scope
  (#eq? @_require "require")
  (#eq? @_router "Router")
  (#eq? @fw.mount.router.ctor "Router"))

; `export const router = express.Router()`: a program-wide binding, and a named
; export of the same name.
((program
  (export_statement
    declaration: (lexical_declaration
      (variable_declarator
        name: (identifier) @fw.mount.router @fw.mount.export.same
        value: (call_expression
          function: [
            (member_expression
              object: [
                (identifier) @fw.mount.router.module
                (call_expression
                  function: (identifier) @_require
                  arguments: (arguments . (string) @fw.mount.router.from .))
              ]
              property: (property_identifier) @_router)
            (identifier) @fw.mount.router.ctor
          ]))))) @fw.mount.scope
  (#eq? @_require "require")
  (#eq? @_router "Router")
  (#eq? @fw.mount.router.ctor "Router"))

; Every other binding hides an outer router of its name over its scope. A
; parameter, across the function's body: `module.exports = function (router) {
; router.get(…) }` registers on whatever router the caller hands in.
(_
  parameters: (formal_parameters
    [
      (required_parameter pattern: (identifier) @fw.mount.shadow)
      (optional_parameter pattern: (identifier) @fw.mount.shadow)
      (required_parameter pattern: (object_pattern (shorthand_property_identifier_pattern) @fw.mount.shadow))
      (required_parameter pattern: (object_pattern (pair_pattern value: (identifier) @fw.mount.shadow)))
    ])
  body: (_) @fw.mount.scope)

(arrow_function
  parameter: (identifier) @fw.mount.shadow
  body: (_) @fw.mount.scope)

; A declaration of any value, plain or destructured, over the block that holds
; it (a router's own declaration is this binding too; the pass keeps its maker).
(_
  [
    (lexical_declaration (variable_declarator name: (identifier) @fw.mount.shadow))
    (variable_declaration (variable_declarator name: (identifier) @fw.mount.shadow))
    (lexical_declaration
      (variable_declarator name: (object_pattern (shorthand_property_identifier_pattern) @fw.mount.shadow)))
    (variable_declaration
      (variable_declarator name: (object_pattern (shorthand_property_identifier_pattern) @fw.mount.shadow)))
    (lexical_declaration
      (variable_declarator name: (object_pattern (pair_pattern value: (identifier) @fw.mount.shadow))))
    (variable_declaration
      (variable_declarator name: (object_pattern (pair_pattern value: (identifier) @fw.mount.shadow))))
  ]) @fw.mount.scope

; A `for … of` / `for … in` variable and a `catch` parameter, over their body.
(for_in_statement
  left: (identifier) @fw.mount.shadow
  body: (_) @fw.mount.scope)

(catch_clause
  parameter: (identifier) @fw.mount.shadow
  body: (_) @fw.mount.scope)

; Imports: `const items = require('./items')`, `const { a, b: c } = require(…)`,
; `import items from './items'`, `import { a, b as c } from …`,
; `import * as m from …`, `import items = require('./items')`.
((variable_declarator
  name: (identifier) @fw.mount.import.name
  value: (call_expression
    function: (identifier) @_require
    arguments: (arguments . (string) @fw.mount.import.module .)))
  (#eq? @_require "require"))

((variable_declarator
  name: (object_pattern
    (shorthand_property_identifier_pattern) @fw.mount.import.name @fw.mount.import.named)
  value: (call_expression
    function: (identifier) @_require
    arguments: (arguments . (string) @fw.mount.import.module .)))
  (#eq? @_require "require"))

((variable_declarator
  name: (object_pattern
    (pair_pattern
      key: (property_identifier) @fw.mount.import.export
      value: (identifier) @fw.mount.import.name))
  value: (call_expression
    function: (identifier) @_require
    arguments: (arguments . (string) @fw.mount.import.module .)))
  (#eq? @_require "require"))

(import_statement
  (import_clause . (identifier) @fw.mount.import.name)
  source: (string) @fw.mount.import.module)

(import_statement
  (import_clause
    (named_imports
      (import_specifier
        name: (identifier) @fw.mount.import.export
        alias: (identifier) @fw.mount.import.name)))
  source: (string) @fw.mount.import.module)

(import_statement
  (import_clause
    (named_imports
      (import_specifier
        name: (identifier) @fw.mount.import.name @fw.mount.import.named
        !alias)))
  source: (string) @fw.mount.import.module)

(import_statement
  (import_clause
    (namespace_import (identifier) @fw.mount.import.name @fw.mount.import.namespace))
  source: (string) @fw.mount.import.module)

(import_statement
  (import_require_clause
    (identifier) @fw.mount.import.name
    source: (string) @fw.mount.import.module))

; Exports: `module.exports = router`, `exports.items = router`,
; `module.exports.items = router`, `export default router`, `export = router`,
; `export { router }`, `export { router as items }`.
((assignment_expression
  left: (member_expression
    object: (identifier) @_module
    property: (property_identifier) @_exports)
  right: (identifier) @fw.mount.export.local)
  (#eq? @_module "module")
  (#eq? @_exports "exports"))

((assignment_expression
  left: (member_expression
    object: (identifier) @_exports
    property: (property_identifier) @fw.mount.export.name)
  right: (identifier) @fw.mount.export.local)
  (#eq? @_exports "exports"))

((assignment_expression
  left: (member_expression
    object: (member_expression
      object: (identifier) @_module
      property: (property_identifier) @_exports)
    property: (property_identifier) @fw.mount.export.name)
  right: (identifier) @fw.mount.export.local)
  (#eq? @_module "module")
  (#eq? @_exports "exports"))

(export_statement
  value: (identifier) @fw.mount.export.local)

; TypeScript's `export = router`, the export `import items = require('./items')` reads.
(export_statement
  "="
  (identifier) @fw.mount.export.local)

(export_statement
  (export_clause
    (export_specifier
      name: (identifier) @fw.mount.export.local
      alias: (identifier) @fw.mount.export.name))
  !source)

(export_statement
  (export_clause
    (export_specifier
      name: (identifier) @fw.mount.export.local @fw.mount.export.same
      !alias))
  !source)

; Mounts: `app.use('/api', router)`, `app.use('/api', auth, router)`,
; `app.use(router)`, `router.use('/v1', require('./v1'))`. One match per argument
; that may be the router, and one for the first argument's shape; the pass merges
; them by the callee.
((call_expression
  function: (member_expression
    object: (_) @fw.mount.receiver
    property: (property_identifier) @fw.mount.verb) @fw.mount.callee
  arguments: (arguments (identifier) @fw.mount.arg))
  (#eq? @fw.mount.verb "use"))

((call_expression
  function: (member_expression
    object: (_) @fw.mount.receiver
    property: (property_identifier) @fw.mount.verb) @fw.mount.callee
  arguments: (arguments
    (call_expression
      function: (identifier) @_require
      arguments: (arguments . (string) @fw.mount.arg.module .))))
  (#eq? @fw.mount.verb "use")
  (#eq? @_require "require"))

((call_expression
  function: (member_expression
    property: (property_identifier) @fw.mount.verb) @fw.mount.callee
  arguments: (arguments . (_) @fw.mount.first))
  (#eq? @fw.mount.verb "use"))

((call_expression
  function: (member_expression
    property: (property_identifier) @fw.mount.verb) @fw.mount.callee
  arguments: (arguments . (string) @fw.mount.path))
  (#eq? @fw.mount.verb "use"))

((call_expression
  function: (member_expression
    property: (property_identifier) @fw.mount.verb) @fw.mount.callee
  arguments: (arguments
    .
    [(call_expression) (arrow_function) (function_expression)] @fw.mount.middleware))
  (#eq? @fw.mount.verb "use"))

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

; ── NestJS decorator routes (S-533, FR-FW-03, FR-FW-05) ──────────────────────
;
; `@Controller("users") class UsersController { @Get(":id") findOne() {} }`.
; Path composition is the shared interpreter's, exactly as for Spring: this file
; CAPTURES the class prefix and the method path and never joins them
; (`resolve::framework::compose_prefixes`). Three things are NestJS's own:
;
;   @fw.route.rooted — NestJS serves every controller path from the root and
;                      writes it without the leading slash (`@Controller("auth")`,
;                      `@Get("me")`), so the interpreter roots a non-empty path or
;                      prefix of the match that carries this capture. It sits on
;                      the decorator-name node of every pattern below, in the
;                      prefix patterns as in the route patterns, because a prefix
;                      and a route are separate matches.
;   @fw.route.prefix.root — the controller states no prefix, so its prefix is `/`
;                      (the empty `@Controller()`), which no literal can say.
;   @fw.route.prefix.absorbs — NestJS strips the end slash of every non-root
;                      path, so `@Get('/')` in `@Controller('users')` is `GET /users`,
;                      the same route `@Get()` is, not `GET /users/`.
;   the decorator position — tree-sitter-typescript puts a METHOD's decorators
;                      in `class_body` as siblings BEFORE its `method_definition`
;                      (not inside it), and a CLASS's decorators on the class — or
;                      on the enclosing `export_statement` when written before
;                      `export`, the usual spelling.
;
; Whether this is NestJS at all is the file-level gate (`framework_detectors`
; names `@nestjs::common`): a `@Get("/x")` in a file that does not import it
; promotes nothing. The decorator name then picks the verb through
; `[framework_methods]` (`Get` → GET … `All` → ANY, capitalised, so Express's
; lower-case table is untouched).
;
; Not applied: a global prefix set in `main.ts` (`app.setGlobalPrefix`) and URI
; versioning (`version: "1"`) — the route is the address the decorators state.
; `@Controller({ version: "1" })`, an options object without a `path`, declares no
; prefix either, but is not read as the root: only the empty call is.
;
; A route decorator is read wherever it sits in a file the gate admits, not only
; in a `@Controller` class: one on an abstract base that a controller extends is
; promoted at its written path, not at the subclass's prefix. And two controllers
; of one file whose handlers share a name get their routes but no handler link
; (the binder's exactly-one rule: a link is never guessed).

; ── The class prefix ─────────────────────────────────────────────────────────
;
; `@Controller()` — an empty call states no prefix and NestJS serves the controller
; from the root, so its prefix is `/` (`@fw.route.prefix.root` on the empty
; argument list, pinned by the text predicate), and so does the empty string,
; `@Controller("")`. A bare `@Get()` in it is then `GET /`, and `@Get("health")`
; is `GET /health`.
;
; Every other form is a wildcard-parent pattern: the declaration that owns the
; decorator is `class_declaration` or the `export_statement` around it, and the
; scope must span decorator AND body, so it is whichever node carries the
; `decorator:` field. A method decorator's parent is `class_body` and a
; parameter decorator's a parameter; neither is named `Controller`, which the
; predicate pins.
((_ decorator: (decorator
     (call_expression
       function: (identifier) @fw.route.prefix.name @fw.route.rooted @fw.route.prefix.absorbs
       arguments: (arguments) @fw.route.prefix.root))) @fw.route.prefix.scope
  (#eq? @fw.route.prefix.name "Controller")
  (#match? @fw.route.prefix.root "^\\([ \\t\\r\\n]*\\)$"))

; Positional literal: `@Controller("users")`, `@Controller(["a", "b"])` (one
; prefix per element, each route fanning out over them).
((_ decorator: (decorator
     (call_expression
       function: (identifier) @fw.route.prefix.name @fw.route.rooted @fw.route.prefix.absorbs
       arguments: (arguments
         [
           (string) @fw.route.prefix
           (array (string) @fw.route.prefix)
         ])))) @fw.route.prefix.scope
  (#eq? @fw.route.prefix.name "Controller"))

; Options object: `@Controller({ path: "auth", version: "1" })`. Only the `path`
; key is a prefix — `version`/`host` hold strings that are not paths.
((_ decorator: (decorator
     (call_expression
       function: (identifier) @fw.route.prefix.name @fw.route.rooted @fw.route.prefix.absorbs
       arguments: (arguments
         (object
           (pair
             key: (property_identifier) @fw.route.key
             value: [
               (string) @fw.route.prefix
               (array (string) @fw.route.prefix)
             ])))))) @fw.route.prefix.scope
  (#eq? @fw.route.prefix.name "Controller")
  (#eq? @fw.route.key "path"))

; A prefix that is not a written literal (`@Controller(PREFIX)`,
; `@Controller(`${base}/x`)`, `{ path: PREFIX }`): captured through the
; `(expression)` supertype — every non-literal form at once — so the shared
; interpreter refuses the routes under it and counts them instead of promoting
; them at a partial path. The supertype also matches a string, which the
; interpreter ignores (an opaque range identical to a captured literal's), and an
; array or object literal — which would disqualify the literals inside it — so
; the `#not-match?` predicates keep those two out: an array is read element by
; element by its own alternative, an options object by the `path:` pattern.
((_ decorator: (decorator
     (call_expression
       function: (identifier) @fw.route.prefix.name @fw.route.rooted @fw.route.prefix.absorbs
       arguments: (arguments
         [
           (expression) @fw.route.prefix.opaque
           (array (expression) @fw.route.prefix.opaque)
         ])))) @fw.route.prefix.scope
  (#eq? @fw.route.prefix.name "Controller")
  (#not-match? @fw.route.prefix.opaque "^[\\[{]"))

((_ decorator: (decorator
     (call_expression
       function: (identifier) @fw.route.prefix.name @fw.route.rooted @fw.route.prefix.absorbs
       arguments: (arguments
         (object
           (pair
             key: (property_identifier) @fw.route.key
             value: [
               (expression) @fw.route.prefix.opaque
               (array (expression) @fw.route.prefix.opaque)
             ])))))) @fw.route.prefix.scope
  (#eq? @fw.route.prefix.name "Controller")
  (#eq? @fw.route.key "path")
  (#not-match? @fw.route.prefix.opaque "^\\["))

; ── The method route ─────────────────────────────────────────────────────────
;
; The route decorator, then any further decorators of the same member
; (`@UseGuards(...)`, `@HttpCode(204)`), then the `method_definition` they all
; decorate. The anchors keep the pairing to ONE member: without them a decorator
; would pair with every later method of the class.

; `@Get("p")`, `@Post(["a", "b"])`.
(class_body
  (decorator
    (call_expression
      function: (identifier) @fw.route.method @fw.route.rooted
      arguments: (arguments
        [
          (string) @fw.route.path
          (array (string) @fw.route.path)
        ]))) @fw.route.anchor
  .
  [(decorator) (comment)]*
  .
  (method_definition
    name: (property_identifier) @fw.route.handler))

; A path that is not a written literal (`@Get(ROUTES.one)`): refused and counted,
; never read as the pathless form below.
((class_body
  (decorator
    (call_expression
      function: (identifier) @fw.route.method @fw.route.rooted
      arguments: (arguments
        [
          (expression) @fw.route.path.opaque
          (array (expression) @fw.route.path.opaque)
        ]))) @fw.route.anchor
  .
  [(decorator) (comment)]*
  .
  (method_definition
    name: (property_identifier) @fw.route.handler))
  (#not-match? @fw.route.path.opaque "^[\\[{]"))

; `@Get()` — no argument at all: the address is the controller prefix itself.
; Pinned to an EMPTY argument list by the text predicate; any argument is one
; of the two patterns above. Unprefixed, the interpreter drops it silently.
((class_body
  (decorator
    (call_expression
      function: (identifier) @fw.route.method @fw.route.rooted
      arguments: (arguments) @fw.route.args)) @fw.route.anchor
  .
  [(decorator) (comment)]*
  .
  (method_definition
    name: (property_identifier) @fw.route.handler))
  (#match? @fw.route.args "^\\([ \\t\\r\\n]*\\)$"))
