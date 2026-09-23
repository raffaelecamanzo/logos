; TSX reference-extraction query (S-015, capability = "references").
;
;   @ref.call   — a plain-identifier call (`f()`).
;   @ref.method — a member call (`obj.m()`, `app.get(...)`); name-only,
;                 policy-gated binding (the Rust `x.f()` posture) — except a
;                 member call on a namespace import (S-440, below).
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
;                 plugin's capture (CR-005, FR-EX-08), against the TSX grammar.
(member_expression
  object: (this)
  property: (property_identifier) @ref.access)
