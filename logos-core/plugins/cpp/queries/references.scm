; C++ reference-extraction query (S-058, capability = "references").
;
;   @ref.method — a call's name (`free()`, `obj.method()`, `obj->method()`,
;                 `ns::func()`); name-only, policy-gated binding (receiver/scope
;                 typing is a resolution concern, deliberately not resolved here).
;   @ref.access — an own-field access (`this->x`): a method reading a field of
;                 its own class (CR-005, FR-EX-08). The receiver-anchored pattern
;                 restricts it to `this->`, so it never double-captures a call;
;                 the enclosing method Contains both endpoints, so resolution
;                 binds an exactly-one Field candidate to an `Accesses` edge
;                 (Method → Field) — the bound LCOM4 input. An ambiguous bare
;                 identifier (a plain `x`, indistinguishable from a local) is
;                 deliberately NOT captured: it would need type resolution C++
;                 cannot give here, so it stays an honest non-edge (NFR-RA-05).
;
; Droppable on disk at `.logos/plugins/cpp/queries/references.scm`.
;
; Deliberately NOT captured in v1: `#include` directives (a header *path*, not a
; `::`-joined symbol path — cross-artifact header resolution is out of scope);
; `new T()` construction (no constructor nodes exist to bind to — see
; symbols.scm); template instantiation and macro-expanded calls (the measured
; precision floor — unbindable constructs yield missing edges, never wrong ones,
; NFR-RA-05).

; A free / unqualified call: `free_call()`.
(call_expression
  function: (identifier) @ref.method)

; A member call: `obj.method()` / `obj->method()` (both are `field_expression`).
(call_expression
  function: (field_expression
    field: (field_identifier) @ref.method))

; A qualified call: `ns::func()` / `Type::static_method()` — the simple name is
; the binding candidate.
(call_expression
  function: (qualified_identifier
    name: (identifier) @ref.method))

; An own-field access through `this->` — the bound LCOM4 / Accesses input.
(field_expression
  argument: (this)
  field: (field_identifier) @ref.access)
