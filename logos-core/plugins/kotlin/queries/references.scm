; Kotlin reference-extraction query (S-055, capability = "references").
;
;   @ref.method — a call's callee name (`callee()`, `service.list()`);
;                 name-only, policy-gated binding (receiver typing is a
;                 resolution concern).
;   @ref.import — an import declaration's qualified path
;                 (`org.springframework.web…`); canonicalised (dots → `::`)
;                 into the ledger form feeding the binder and the framework
;                 candidacy gate (FR-FW-04).
;   @ref.access — an own-property access (`this.x`): the bound LCOM4 input
;                 (Method → Field), the same structural pattern as Java's
;                 `this.<field>` access (CR-005, FR-EX-08).
;
; Droppable on disk at `.logos/plugins/kotlin/queries/references.scm`.
;
; Deliberately NOT captured in v1: constructor calls (`ClassName()` shares its
; class's name — no constructor node to bind to, so `new`-style references stay
; honestly unresolved), extension-function dispatch, and member binding of
; star-imports.

; A receiver-less call: `callee()` — the callee is the call's first expression
; child, here a bare identifier. (Arguments live under `value_arguments`, never
; as a direct identifier child, so this captures only the callee.)
(call_expression
  (identifier) @ref.method)

; A receiver method call: `service.list()` — the member name after navigation.
(call_expression
  (navigation_expression
    (identifier) @ref.method))

; An import's qualified path — canonicalised (dots → `::`) into the ledger form
; that feeds the binder and the Spring candidacy gate.
(import
  (qualified_identifier) @ref.import)

; An own-property access `this.x`: a method reading a property of its own class.
; The class lexically Contains both the method and its `property` declarations
; (symbols.scm captures Kotlin properties as Field), so resolution binds an
; exactly-one Field candidate to an `Accesses` edge (Method → Field); an
; ambiguous access stays unresolved (NFR-RA-05). This is the bound LCOM4 input.
(navigation_expression
  (this_expression)
  (identifier) @ref.access)
