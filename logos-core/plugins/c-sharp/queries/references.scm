; C# reference-extraction query (S-057, CR-009, capability = "references").
;
;   @ref.method — a method invocation's name (`service.List()`, `List()`);
;                 name-only, policy-gated binding (receiver typing is a
;                 resolution concern).
;   @ref.import — a `using` directive's namespace path (`Microsoft.AspNetCore.Mvc`);
;                 canonicalised (dots → `::`) into the ledger form feeding the
;                 binder and the framework candidacy gate (FR-FW-04).
;   @ref.access — an own-field access (`this.X`): a method reading a field of its
;                 own type (CR-005, FR-EX-08); the bound LCOM4 input.
;
; Droppable on disk at `.logos/plugins/c-sharp/queries/references.scm`.
;
; Deliberately NOT captured in v1: `new T()` construction references (no
; constructor nodes exist to bind them to — see symbols.scm), `using static`
; member binding, generic type arguments.

; Bare call (`List()`) and member call (`service.List()`): the method name.
(invocation_expression
  function: (identifier) @ref.method)

(invocation_expression
  function: (member_access_expression
    name: (identifier) @ref.method))

; `using System.Collections.Generic;` / `using Microsoft.AspNetCore.Mvc;` — the
; namespace path (qualified) or a single-segment namespace.
(using_directive
  (qualified_name) @ref.import)

(using_directive
  (identifier) @ref.import)

; Own-field access (`this.Count`): the binder proves an exactly-one Field
; candidate in the enclosing type for an `Accesses` edge (Method → Field); an
; ambiguous access stays unresolved (NFR-RA-05). `this` is an anonymous keyword
; token in tree-sitter-c-sharp, so it is matched as a string, not a node.
(member_access_expression
  expression: "this"
  name: (identifier) @ref.access)
