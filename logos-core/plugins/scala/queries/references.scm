; Scala reference-extraction query (S-061, CR-009, capability = "references").
;
;   @ref.method — a method/function invocation's name, by simple name
;                 (`compute()`) or as the selected member of a receiver
;                 (`helper.doThing()`). A Method-form row that binds by its
;                 receiver's SHAPE, recorded by the `@ref.receiver.*` markers
;                 below (S-514, S-516, CR-169), yielding a `Calls` edge.
;   @ref.access — an own-field access (`this.x`): a method reading a field of
;                 its own class (CR-005, FR-EX-08), the bound LCOM4 input. The
;                 class lexically Contains both the method and its `val`/`var`
;                 members, so resolution binds an exactly-one Field candidate to
;                 an `Accesses` edge; an ambiguous/unmatched access stays
;                 unresolved (NFR-RA-05).
;
; Receiver-shape markers (S-516, FR-EX-13, FR-RS-12) — they record no row of
; their own; each names the receiver of the `@ref.method` that shares its parent
; node (the `field_expression`, or for a bare call the `call_expression`).
; `this` and `super` parse as plain identifiers, so they are told apart by text:
;
;   @ref.receiver.self      — `this.compute()`: binds among the enclosing
;                             class's, object's or trait's own members only.
;                             A qualified `Outer.this` names another instance,
;                             so it is `other`.
;   @ref.receiver.super     — `super.compute()`: binds only through a proven
;                             `Extends`; Scala records none, so it stays
;                             unbound.
;   @ref.receiver.other     — every other receiver (`helper.doThing()`): never
;                             bound through the caller's scope
;                             (`no-receiver-evidence`).
;   @ref.receiver.implicit  — `compute()`, no receiver: under this plugin's
;                             `implicit_receiver = "self"` a call on the current
;                             instance inside a class, object or trait, a free
;                             call outside one (a Scala 3 top-level `def`).
;   @ref.receiver.anonymous — a `new T { … }` body: its instance has no node,
;                             so a `this.` call inside records `other` and a
;                             bare call is a free call.
;
; Droppable on disk at `.logos/plugins/scala/queries/references.scm`.
;
;   @ref.import.dotted — an `import` declaration (S-518, CR-170, FR-RS-13):
;                 `import a.b.C`, `import a.b._` / `a.b.*`, `import a.b.{C, D}`
;                 and comma-separated paths. Scala flattens a dotted path into
;                 repeated `path:` identifiers with no single spanning node, so
;                 the whole declaration is captured and walked structurally
;                 (`extract::refs::flatten_dotted_import`, the twin of Rust's
;                 `ref.use` walk): one row per imported path, a wildcard a
;                 `Glob` row. The binder resolves each through the declared
;                 package's fully-qualified type index.

; A call by simple name: `compute()`, `assert(...)`, the `test`/`it` markers.
; An auxiliary constructor's `def this(a: Int) = this(a, 0)` delegates to the
; primary constructor, which has no node: read as a call named `this`, it would
; bind the auxiliary constructor to itself, so it is not captured.
((call_expression
  function: (identifier) @ref.method @ref.receiver.implicit)
  (#not-eq? @ref.method "this"))

; A call selecting a member of a receiver: `helper.doThing()` — the method name.
(call_expression
  function: (field_expression
    field: (identifier) @ref.method))

; The receiver of a member call.
((field_expression
  value: (identifier) @ref.receiver.self)
  (#eq? @ref.receiver.self "this"))

((field_expression
  value: (identifier) @ref.receiver.super)
  (#eq? @ref.receiver.super "super"))

(field_expression
  value: (_) @ref.receiver.other)

(instance_expression
  (template_body) @ref.receiver.anonymous)

; An own-field access `this.x` (the bound cohesion input). Over-capturing a
; `this.m()` method call here is harmless: with no Field named `m` in the class,
; the Accesses candidate simply never binds (NFR-RA-05).
(
  (field_expression
    value: (identifier) @_recv
    field: (identifier) @ref.access)
  (#eq? @_recv "this")
)

; An import declaration, walked into one row per imported path (S-518).
(import_declaration) @ref.import.dotted

; ── Argument count (S-591, CR-190, FR-EX-32) ─────────────────────────────────
; The `@arity.*` vocabulary the extraction engine reads (`extract::arity`): every
; argument list, whose named children a call row counts, and the forms that
; make a count unknown. Captures record no row of their own, so every ledger
; target is unchanged.
; A call counts its first argument list only; a block or case block written as
; the list (`f { … }`) is one argument. `xs: _*` and `xs*` spread a sequence.
(arguments) @arity.arguments
(call_expression
  arguments: [(block) (case_block)] @arity.block)
(call_expression
  arguments: (colon_argument) @arity.opaque)
(arguments (ascription_expression (repeated_parameter_type)) @arity.spread)
(arguments
  (postfix_expression (operator_identifier) @_spread) @arity.spread
  (#eq? @_spread "*"))
