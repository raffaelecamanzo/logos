; C++ reference-extraction query (S-058, capability = "references").
;
;   @ref.method — a call's name (`free()`, `obj.method()`, `obj->method()`,
;                 `ns::func()`); a Method-form row that binds by its receiver's
;                 SHAPE, recorded by the `@ref.receiver.*` markers below (S-514,
;                 S-516, CR-169).
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
; Receiver-shape markers (S-516, FR-EX-13, FR-RS-12) — they record no row of
; their own; each names the receiver of the `@ref.method` that shares its parent
; node (the `field_expression` / `qualified_identifier`, or for a bare call the
; `call_expression`):
;
;   @ref.receiver.self     — `this->method()`, `(*this).method()`: binds among
;                            the enclosing class's own members only.
;   @ref.receiver.other    — every other receiver (`obj.method()`,
;                            `ptr->method()`), and every qualifying scope
;                            (`ns::func()`, `Type::method()`, `Base::method()`):
;                            never bound through the caller's scope
;                            (`no-receiver-evidence`). C++ has no `super`
;                            keyword, and a base-class qualifier is the same
;                            syntax as a namespace one, so nothing is `super`.
;   @ref.receiver.implicit — `method()`, no receiver: under this plugin's
;                            `implicit_receiver = "self"` a call on the current
;                            instance inside a class or struct body, a free call
;                            outside one (a free function, or an out-of-line
;                            `Type::method` definition, which no declaration
;                            here encloses).
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
  function: (identifier) @ref.method @ref.receiver.implicit)

; A member call: `obj.method()` / `obj->method()` (both are `field_expression`).
(call_expression
  function: (field_expression
    field: (field_identifier) @ref.method))

; A qualified call: `ns::func()` / `Type::static_method()` — the simple name is
; the binding candidate.
(call_expression
  function: (qualified_identifier
    name: (identifier) @ref.method))

; The receiver of a member call, and the scope of a qualified one.
(field_expression
  argument: (this) @ref.receiver.self)

(field_expression
  argument: (parenthesized_expression
    (pointer_expression
      argument: (this))) @ref.receiver.self)

(field_expression
  argument: (_) @ref.receiver.other)

(qualified_identifier
  scope: (_) @ref.receiver.other)

; An own-field access through `this->` — the bound LCOM4 / Accesses input.
(field_expression
  argument: (this)
  field: (field_identifier) @ref.access)

; ── Argument count (S-591, CR-190, FR-EX-32) ─────────────────────────────────
; The `@arity.*` vocabulary the extraction engine reads (`extract::arity`): every
; argument list, whose named children a call row counts, and the forms that
; make a count unknown. Captures record no row of their own, so every ledger
; target is unchanged.
; A pack expansion (`xs...`) spreads; a braced `new T{…}` is not counted.
(argument_list) @arity.arguments
(argument_list (parameter_pack_expansion) @arity.spread)
(new_expression
  arguments: (initializer_list) @arity.opaque)
