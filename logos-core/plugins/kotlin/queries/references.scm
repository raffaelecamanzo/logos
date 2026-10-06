; Kotlin reference-extraction query (S-055, capability = "references").
;
;   @ref.method — a call's callee name (`callee()`, `service.list()`); a
;                 Method-form row that binds by its receiver's SHAPE, recorded
;                 by the `@ref.receiver.*` markers below (S-514, S-516, CR-169).
;   @ref.import — an import declaration's qualified path
;                 (`org.springframework.web…`); canonicalised (dots → `::`)
;                 into the ledger form feeding the binder and the framework
;                 candidacy gate (FR-FW-04).
;   @ref.import.alias — the local name an `import a.b.C as D` binds (`D`;
;                 S-520). The row's alias is that name, not `C` — which the
;                 import does not bring into view, so a same-package `C` is
;                 still the one an unqualified `C` names; without an `as` it
;                 stays the path's last segment.
;   @ref.access — an own-property access (`this.x`): the bound LCOM4 input
;                 (Method → Field), the same structural pattern as Java's
;                 `this.<field>` access (CR-005, FR-EX-08).
;   @ref.extends — each supertype a class, interface or object lists after `:`
;                 (`class A : Base(), Iface` → `Base`, `Iface`; S-522,
;                 FR-RS-15), whether invoked as the base class's constructor,
;                 delegated (`Iface by impl`) or bare. The list does not say
;                 which entry is the class, so the plugin declares
;                 `supertype_kind_follows_target`: a class's entry binds the one
;                 in-repository class or interface it names, through the
;                 package rungs, and its edge is `Extends` to a class,
;                 `Implements` to an interface. An interface's entries are its
;                 super-interfaces (`Extends`).
;
; Receiver-shape markers (S-516, FR-EX-13, FR-RS-12) — they record no row of
; their own; each names the receiver of the `@ref.method` that shares its parent
; node (the `navigation_expression`, or for a bare call the `call_expression`):
;
;   @ref.receiver.self      — `this.list()`: binds among the enclosing class's
;                             own members only. A labelled `this@Outer` names
;                             another instance, so it is `other`.
;   @ref.receiver.super     — `super.list()`: binds only through a proven
;                             `Extends` of the caller's class — its base class,
;                             never an interface (`@ref.extends` below).
;                             `super<T>.list()` names the supertype `T` and
;                             `super@Outer.list()` an outer class's, neither of
;                             which the caller's own hierarchy decides: both are
;                             `other`.
;   @ref.receiver.other     — every other receiver (`service.list()`,
;                             `a.b.list()`): never bound through the caller's
;                             scope (`no-receiver-evidence`).
;   @ref.receiver.implicit  — `list()`, no receiver: under this plugin's
;                             `implicit_receiver = "self"` a call on the current
;                             instance inside a class or object, a free call
;                             outside one.
;   @ref.receiver.anonymous — an `object : T { … }` body, and an extension
;                             function's body (`fun G.ext() { … }`, whose `this`
;                             is a `G`): its receiver has no node to bind
;                             through, so a `this.` call inside records `other`
;                             and a bare call is a free call.
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

((navigation_expression
  .
  (super_expression) @ref.receiver.super)
  (#eq? @ref.receiver.super "super"))

(navigation_expression
  .
  (_) @ref.receiver.other)

(object_literal
  (class_body) @ref.receiver.anonymous)

; An extension function's body: its `this` is the extension receiver (`G` in
; `fun G.ext()`), not the enclosing class's instance, so a `this.` call inside
; it must not bind among that class's members. The receiver type sits right
; before the name; a return type follows the parameters, so the anchor tells
; the two apart.
(function_declaration
  [(user_type) (nullable_type)]
  .
  name: (identifier)
  (function_body) @ref.receiver.anonymous)

; An import's qualified path — canonicalised (dots → `::`) into the ledger form
; that feeds the binder and the Spring candidacy gate. `import a.b.C` (and its
; aliased form) names one declaration; `import a.b.*` brings every type of the
; package `a.b` into view, so it is marked a wildcard (`@ref.import.asterisk`,
; S-518) and records no alias. The two are told apart by the trailing `*`, so
; exactly one pattern matches each import. An aliased import's `identifier`
; after `as` is its local name (`@ref.import.alias`).
((import
  (qualified_identifier) @ref.import
  (identifier)? @ref.import.alias) @_import
  (#not-match? @_import "\\*\\s*;?\\s*$"))

(import
  (qualified_identifier) @ref.import
  "*" @ref.import.asterisk)

; An own-property access `this.x`: a method reading a property of its own class.
; The class lexically Contains both the method and its `property` declarations
; (symbols.scm captures Kotlin properties as Field), so resolution binds an
; exactly-one Field candidate to an `Accesses` edge (Method → Field); an
; ambiguous access stays unresolved (NFR-RA-05). This is the bound LCOM4 input.
(navigation_expression
  (this_expression)
  (identifier) @ref.access)

;   @ref.extends — anchored to the declarations that own a supertype list. A
;   companion object's and an object expression's (`object : T { … }`) have no
;   node of their own, so they are never read as the enclosing declaration's.
(class_declaration
  (delegation_specifiers
    (delegation_specifier
      [(user_type) @ref.extends
       (constructor_invocation (user_type) @ref.extends)
       (explicit_delegation (user_type) @ref.extends)])))
(object_declaration
  (delegation_specifiers
    (delegation_specifier
      [(user_type) @ref.extends
       (constructor_invocation (user_type) @ref.extends)
       (explicit_delegation (user_type) @ref.extends)])))

; ── Argument count (S-591, CR-190, FR-EX-32) ─────────────────────────────────
; The `@arity.*` vocabulary the extraction engine reads (`extract::arity`): every
; argument list, whose named children a call row counts, and the forms that
; make a count unknown. Captures record no row of their own, so every ledger
; target is unchanged.
; A trailing lambda (`f(1) { … }`, `f { … }`) is one more argument; `*xs`
; spreads an array into a vararg.
(value_arguments) @arity.arguments
(annotated_lambda) @arity.lambda
(value_arguments (value_argument (spread_expression)) @arity.spread)
