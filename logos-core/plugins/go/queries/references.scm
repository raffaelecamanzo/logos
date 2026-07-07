; Go reference-extraction query (S-015, capability = "references").
;
;   @ref.call   — a plain-identifier call (`f()`).
;   @ref.method — a selector call (`pkg.F()`, `recv.M()`); name-only,
;                 policy-gated binding (package member vs receiver method is
;                 a resolution concern).
;   @ref.import — an import path string (`import "net/http"`); unquoted and
;                 canonicalised (slashes → `::`) into the ledger form feeding
;                 the binder and the framework candidacy gate (FR-FW-04).
;
; Droppable on disk at `.logos/plugins/go/queries/references.scm`.

(call_expression
  function: (identifier) @ref.call)

(call_expression
  function: (selector_expression
    field: (field_identifier) @ref.method))

(import_spec
  path: (interpreted_string_literal) @ref.import)
