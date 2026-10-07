; C reference-extraction query (S-056, capability = "references").
;
;   @ref.call — a direct function call (`f()`); bound by the scope-hierarchy
;               rules to a same-translation-unit definition, else left honestly
;               unresolved and retried on sync (NFR-RA-05).
;
; Droppable on disk at `.logos/plugins/c/queries/references.scm`. Deliberately
; NOT captured in v1: `#include` directives (header ownership belongs to the C++
; plugin under the fixed `.h` rule, S-058, so a C-only repo would only ever leave
; them unresolved) and indirect calls through function pointers (no statically
; bindable callee — never fabricated).
(call_expression
  function: (identifier) @ref.call)

; ── Argument count (S-591, CR-190, FR-EX-32) ─────────────────────────────────
; The `@arity.*` vocabulary the extraction engine reads (`extract::arity`): every
; argument list, whose named children a call row counts, and the forms that
; make a count unknown. Captures record no row of their own, so every ledger
; target is unchanged.
(argument_list) @arity.arguments
