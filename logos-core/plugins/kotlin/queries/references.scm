; Kotlin reference-extraction query (S-055, capability = "references").
;
;   @ref.method — a call's callee name (`callee()`, `service.list()`); a
;                 Method-form row that binds by its receiver's SHAPE, recorded
;                 by the `@ref.receiver.*` markers below (S-514, S-516, CR-169).
;   @ref.import — an import declaration's qualified path
;                 (`org.springframework.web…`); canonicalised (dots → `::`)
;                 into the ledger form feeding the binder and the framework
;                 candidacy gate (FR-FW-04).
;   @ref.access — an own-property access (`this.x`): the bound LCOM4 input
;                 (Method → Field), the same structural pattern as Java's
;                 `this.<field>` access (CR-005, FR-EX-08).
;
; Receiver-shape markers (S-516, FR-EX-13, FR-RS-12) — they record no row of
; their own; each names the receiver of the `@ref.method` that shares its parent
; node (the `navigation_expression`, or for a bare call the `call_expression`):
;
;   @ref.receiver.self      — `this.list()`: binds among the enclosing class's
;                             own members only. A labelled `this@Outer` names
;                             another instance, so it is `other`.
;   @ref.receiver.super     — `super.list()`, `super<T>.list()`: binds only
;                             through a proven `Extends`; Kotlin records none,
;                             so it stays unbound.
;   @ref.receiver.other     — every other receiver (`service.list()`,
;                             `a.b.list()`): never bound through the caller's
;                             scope (`no-receiver-evidence`).
;   @ref.receiver.implicit  — `list()`, no receiver: under this plugin's
;                             `implicit_receiver = "self"` a call on the current
;                             instance inside a class or object, a free call
;                             outside one.
;   @ref.receiver.anonymous — an `object : T { … }` body: its instance has no
;                             node, so a `this.` call inside records `other` and
;                             a bare call is a free call.
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
  (identifier) @ref.method @ref.receiver.implicit)

; A receiver method call: `service.list()` — the member name after navigation.
; `navigation_expression` has no fields and its receiver may itself be a bare
; `identifier`, so the member is anchored as the navigation's LAST child: in
; `a.b.c()` only `c` is a call, never the receiver `a` (KOTLIN-G4).
(call_expression
  (navigation_expression
    (identifier) @ref.method .))

; The receiver of a member call: the navigation's first child.
((navigation_expression
  .
  (this_expression) @ref.receiver.self)
  (#eq? @ref.receiver.self "this"))

(navigation_expression
  .
  (super_expression) @ref.receiver.super)

(navigation_expression
  .
  (_) @ref.receiver.other)

(object_literal
  (class_body) @ref.receiver.anonymous)

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
