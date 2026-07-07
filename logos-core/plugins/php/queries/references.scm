; PHP reference-extraction query (S-060, capability = "references").
;
;   @ref.call   — a free function call's name (`callee()`); name-only,
;                 policy-gated binding.
;   @ref.method — an instance (`$svc->list()`) or static (`Foo::bar()`) method
;                 call name; name-only (receiver typing is a resolution concern).
;   @ref.import — a `use` clause's namespace path (`Illuminate\Support\…\Route`);
;                 canonicalised (backslash → `::`) into the ledger form feeding
;                 the binder and the framework candidacy gate (FR-FW-04).
;   @ref.access — an own-property access (`$this->balance`): a method reading a
;                 field of its own class (CR-005, FR-EX-08), the bound LCOM4 input.
;
; Droppable on disk at `.logos/plugins/php/queries/references.scm`.
;
; Deliberately NOT captured in v1: `new Class()` construction references (no
; constructor symbol to bind them — see symbols.scm), dynamic calls
; (`$obj->$method()`, `call_user_func`), variable-variable indirection — they
; stay honestly unresolved (NFR-RA-05, never fabricate).

(function_call_expression
  function: (name) @ref.call)

(member_call_expression
  name: (name) @ref.method)

(scoped_call_expression
  name: (name) @ref.method)

(namespace_use_clause
  (qualified_name) @ref.import)

;   @ref.access — `$this->field`: an own-field read/write. `member_access_expression`
; is a distinct node from `member_call_expression`, so this never double-captures
; a method call. The class lexically Contains both the method and its
; `property_declaration` fields, so resolution binds an exactly-one Field
; candidate to an `Accesses` edge (Method → Field); an ambiguous access stays
; unresolved (NFR-RA-05).
((member_access_expression
  object: (variable_name (name) @_recv)
  name: (name) @ref.access)
  (#eq? @_recv "this"))
