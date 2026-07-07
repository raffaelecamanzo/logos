; TSX reference-extraction query (S-015, capability = "references").
;
;   @ref.call   — a plain-identifier call (`f()`).
;   @ref.method — a member call (`obj.m()`, `app.get(...)`); name-only,
;                 policy-gated binding (the Rust `x.f()` posture).
;   @ref.import — an import source string (`import x from "express"`) or a
;                 CommonJS `require("...")` argument; the quoted text is
;                 unquoted and canonicalised into the `::`-joined ledger form
;                 feeding the binder and the framework candidacy gate
;                 (FR-FW-04).
;
; Droppable on disk at `.logos/plugins/tsx/queries/references.scm`.
;
; Deliberately NOT captured in v1: named-import bindings (`import { Router }`
; binds no per-name alias yet), dynamic `import()`.

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

;   @ref.access — an own-field access (`this.x`): identical to the typescript
;                 plugin's capture (CR-005, FR-EX-08), against the TSX grammar.
(member_expression
  object: (this)
  property: (property_identifier) @ref.access)
