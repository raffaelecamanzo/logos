; Java reference-extraction query (S-015, capability = "references").
;
;   @ref.method — a method invocation's name (`service.list()`, `list()`);
;                 name-only, policy-gated binding (receiver typing is a
;                 resolution concern).
;   @ref.import — an import declaration's scoped path
;                 (`org.springframework.web…`); canonicalised (dots → `::`)
;                 into the ledger form feeding the binder and the framework
;                 candidacy gate (FR-FW-04).
;
; Droppable on disk at `.logos/plugins/java/queries/references.scm`.
;
; Deliberately NOT captured in v1: `new T()` construction references (no
; constructor nodes exist to bind them to — see symbols.scm), static wildcard
; imports' member binding.

(method_invocation
  name: (identifier) @ref.method)

(import_declaration
  (scoped_identifier) @ref.import)

;   @ref.access — an own-field access (`this.x`): a method reading a field of
;                 its own class (CR-005, FR-EX-08). `field_access` is a distinct
;                 node from `method_invocation`, so this never double-captures a
;                 method call. The class lexically Contains both the method and
;                 its `field` declarations (symbols.scm captures Java fields), so
;                 resolution binds an exactly-one Field candidate to an
;                 `Accesses` edge (Method → Field); an ambiguous access stays
;                 unresolved (NFR-RA-05). This is the bound LCOM4 input.
(field_access
  object: (this)
  field: (identifier) @ref.access)
