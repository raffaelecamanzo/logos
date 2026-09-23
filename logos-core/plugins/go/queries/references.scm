; Go reference-extraction query (S-015, capability = "references").
;
;   @ref.call   — a plain-identifier call (`f()`).
;   @ref.method — a selector call (`pkg.F()`, `recv.M()`). A call on a value
;                 (`recv.M()`) stays name-only, policy-gated (FR-RS-06); a
;                 call whose operand is an imported package (`pkg.F()`) is
;                 recorded qualified by its import path by the extraction
;                 engine itself (S-440).
;   @ref.import — an import path string (`import "net/http"`); unquoted and
;                 canonicalised by the PATH grammar the descriptor declares
;                 (only slashes → `::`; a host's dots are kept, S-439) into
;                 the ledger form feeding the binder and the framework
;                 candidacy gate (FR-FW-04).
;
; Droppable on disk at `.logos/plugins/go/queries/references.scm`.

(call_expression
  function: (identifier) @ref.call)

(call_expression
  function: (selector_expression
    field: (field_identifier) @ref.method))

(import_spec
  path: (interpreted_string_literal) @ref.import)
