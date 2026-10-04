; C# reference-extraction query (S-057, CR-009, capability = "references").
;
;   @ref.method — a method invocation's name (`service.List()`, `List()`); a
;                 Method-form row that binds by its receiver's SHAPE, recorded
;                 by the `@ref.receiver.*` markers below (S-514, S-516, CR-169).
;   @ref.import — a `using` directive's namespace path (`Microsoft.AspNetCore.Mvc`);
;                 canonicalised (dots → `::`) into the ledger form feeding the
;                 binder and the framework candidacy gate (FR-FW-04).
;   @ref.access — an own-field access (`this.X`): a method reading a field of its
;                 own type (CR-005, FR-EX-08); the bound LCOM4 input.
;
; Receiver-shape markers (S-516, FR-EX-13, FR-RS-12) — they record no row of
; their own; each names the receiver of the `@ref.method` that shares its parent
; node (the `member_access_expression`, or for a bare call the invocation):
;
;   @ref.receiver.self     — `this.List()`: binds among the enclosing class's
;                            own members only.
;   @ref.receiver.super    — `base.List()`: binds only through a proven
;                            `Extends`; C# records none, so it stays unbound.
;   @ref.receiver.other    — every other receiver (`service.List()`,
;                            `Enumerable.Range()`): never bound through the
;                            caller's scope (`no-receiver-evidence`).
;   @ref.receiver.implicit — `List()`, no receiver: under this plugin's
;                            `implicit_receiver = "self"` a call on the current
;                            instance inside a type, a free call outside one.
;
; Droppable on disk at `.logos/plugins/c-sharp/queries/references.scm`.
;
; Deliberately NOT captured in v1: `new T()` construction references (no
; constructor nodes exist to bind them to — see symbols.scm), `using static`
; member binding, generic type arguments, and the generic (`M<T>()`) and
; conditional (`x?.M()`) invocation forms.

; Bare call (`List()`): no receiver written.
(invocation_expression
  function: (identifier) @ref.method @ref.receiver.implicit)

; Member call (`service.List()`, `this.List()`, `base.List()`): the method name.
(invocation_expression
  function: (member_access_expression
    name: (identifier) @ref.method))

; The receiver of a member call. `this` and `base` are anonymous keyword tokens
; in tree-sitter-c-sharp, matched as strings; `(_)` matches named nodes only,
; so the catch-all never reaches either keyword.
(member_access_expression
  expression: "this" @ref.receiver.self)

(member_access_expression
  expression: "base" @ref.receiver.super)

(member_access_expression
  expression: (_) @ref.receiver.other)

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
