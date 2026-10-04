; TSX reference-extraction query (S-015, capability = "references").
;
;   @ref.call   — a plain-identifier call (`f()`).
;   @ref.method — a member call (`obj.m()`, `app.get(...)`): the name alone,
;                 bound by its receiver's SHAPE (the markers below) — except a
;                 member call on a namespace import (S-440, below), recorded
;                 as the module-qualified path instead.
;   @ref.import — an import source string (`import x from "express"`) or a
;                 CommonJS `require("...")` argument; the quoted text is
;                 unquoted and canonicalised by the PATH grammar the
;                 descriptor declares (S-439) into the `::`-joined ledger
;                 form feeding the binder and the framework candidacy gate
;                 (FR-FW-04).
;
; Droppable on disk at `.logos/plugins/tsx/queries/references.scm`.
;
; Named-import bindings (`import { Router } from './r'`) and namespace imports
; (`import * as m from './m'`) are read from the `@ref.import` statement by the
; extraction engine itself (S-440), which records a call through one — `Router()`,
; `m.f()` — qualified by its module. Not captured: dynamic `import()`.

(call_expression
  function: (identifier) @ref.call)

(call_expression
  function: (member_expression
    property: (property_identifier) @ref.method))

;   Receiver markers (S-515, CR-169, FR-EX-13) — MARKERS, recording nothing on
;   their own. Each names the receiver of the member call above: its captured
;   node shares the `member_expression` the `@ref.method` name sits in, and
;   the call records that SHAPE, which the binder dispatches on (FR-RS-12).
;   `.js` sources parse with this grammar, so JavaScript is covered too.
;
;   @ref.receiver.self      — `this.m()`: binds among the enclosing class's own
;                             members only.
;   @ref.receiver.super     — `super.m()`: binds only through a proven
;                             `Extends`, which this plugin does not record — so
;                             it stays unbound, and never reaches the caller's
;                             own class.
;   @ref.receiver.other     — every receiver (`obj.m()`, `this.x.m()`, a chained
;                             call): never binds. `self`/`super` outrank it.
;   @ref.receiver.anonymous — a body whose `this` is not the enclosing class's
;                             instance, so a `this.m()` inside is never that
;                             class's `m`: a class EXPRESSION's body (`return
;                             class { … }`, no class node — `other`, unless the
;                             body declares `m` itself), and the bodies that
;                             REBIND `this` — an object-literal method and a
;                             non-arrow `function` (`other`). An arrow function
;                             keeps the method's `this`, so it is not marked.
;
;   An unqualified call (`m()`) is never a call on the instance in TS/JS: it is
;   the free `@ref.call` above (`implicit_receiver` stays "none").
(call_expression
  function: (member_expression object: (this) @ref.receiver.self))
(call_expression
  function: (member_expression object: (super) @ref.receiver.super))
(call_expression
  function: (member_expression object: (_) @ref.receiver.other))
(class body: (class_body) @ref.receiver.anonymous)
(object (method_definition) @ref.receiver.anonymous)
[(function_expression) (function_declaration) (generator_function)
 (generator_function_declaration)] @ref.receiver.anonymous

(import_statement
  source: (string) @ref.import)

((call_expression
  function: (identifier) @_require
  arguments: (arguments
    .
    (string) @ref.import))
  (#eq? @_require "require"))

;   @ref.call on a JSX element (S-440, CR-142) — `<RuleFindingsCard />` is how a
;                 TSX file calls a component, so it is a call of the component.
;                 Only a capitalised plain identifier: a lower-case tag (`<div>`)
;                 is an intrinsic element and names no workspace symbol, and a
;                 member tag (`<Foo.Bar>`) is not a plain name.
(jsx_opening_element
  name: (identifier) @ref.call
  (#match? @ref.call "^[A-Z]"))

(jsx_self_closing_element
  name: (identifier) @ref.call
  (#match? @ref.call "^[A-Z]"))

;   @ref.access — an own-field access (`this.x`): identical to the typescript
;                 plugin's capture (CR-005, FR-EX-08), against the TSX grammar —
;                 including `this.#x`, a `private_property_identifier` (S-477).
(member_expression
  object: (this)
  property: [(property_identifier) (private_property_identifier)] @ref.access)
