; Rust reference-extraction query (S-011, capability = "references").
;
; Captures the *outgoing references* of a file — the raw material of the
; resolution pass (Pass 2). Where `symbols.scm` captures what a file declares,
; this file captures what it points at: call paths, receiver-method calls, and
; `use` imports. Extraction turns each capture into a RefFact persisted in the
; `unresolved_refs` ledger; the resolution engine then binds each ref by the
; scope-hierarchy rules — or leaves it honestly unresolved (NFR-RA-05).
;
; The capture name after the `@` carries the reference shape:
;   @ref.call   — a path call (`f()`, `a::b::f()`, with or without turbofish);
;                 the captured node's text is the language path to resolve.
;   @ref.method — a receiver-method call (`x.f()`); only the method *name* is
;                 knowable without type inference. The `@ref.receiver.other`
;                 marker below records its shape `other` (S-517), so it binds
;                 nowhere under any policy (S-514, FR-RS-12). Where the file
;                 proves the receiver's declared type `T` (the receiver-typing
;                 markers below, S-587), the row is `T::f` in Path form instead,
;                 still of shape `other`.
;   @ref.method.self — a receiver-method call on exactly `self` (`self.f()`),
;                 whose receiver type is the enclosing impl's self type (S-493);
;                 see its pattern below.
;   @ref.use    — a whole `use` declaration argument; extraction walks the use
;                 tree (groups, `as` renames, globs) in code, since a query
;                 cannot flatten arbitrary nesting.
;   @ref.macro  — a whole `macro_invocation`; tree-sitter does not parse a
;                 macro's token tree as expressions, so a query cannot match the
;                 call/method calls nested inside it. Extraction walks the token
;                 tree in code (S-162, CR-043) and emits the same Calls
;                 path/method RefFacts, so a callee whose only call site is a
;                 macro argument (`format!("{x}", x = activity_card(s))`) is no
;                 longer mis-bound dead; a `self.f()` there records what
;                 `@ref.method.self` records (S-514), and any other method call
;                 there records `other`, as `@ref.receiver.other` does outside
;                 a macro (S-517).
;
; Like every capability query, this file is droppable-on-disk: a copy at
; `.logos/plugins/rust/queries/references.scm` shadows it without a rebuild
; (FR-PL-04, FR-PL-05).
;
; Deliberately NOT captured (documented limitations, S-011):
;   - bare type mentions / struct-literal instantiations (References /
;     Instantiates edges are a later increment).
; Macro-token-tree calls WERE a v1 limitation; S-162 (CR-043) lifts it for the
; Calls relation via the `@ref.macro` capture above.

(call_expression
  function: (identifier) @ref.call)

(call_expression
  function: (scoped_identifier) @ref.call)

(call_expression
  function: (generic_function
    function: (identifier) @ref.call))

(call_expression
  function: (generic_function
    function: (scoped_identifier) @ref.call))

(call_expression
  function: (field_expression
    value: (_) @_receiver
    field: (field_identifier) @ref.method)
  (#not-eq? @_receiver "self"))

;   @ref.method.self — a receiver-method call whose receiver is exactly `self`
;                 (S-493, FR-RS-11). Its type is the enclosing impl's self type
;                 (`symbols.scm`'s `@symbol.self_type`), so extraction records it
;                 as the Path-form `Self::f` — the row a written `Self::f()`
;                 records — and the binder binds it among that type's methods in
;                 the caller's crate. The `#not-eq?` above keeps `@ref.method` off
;                 these calls, so each is captured once. `self.field.f()` and
;                 `other.f()` are `@ref.method`. It is a `self`-marked
;                 `@ref.method` (S-514): inside a trait's default method (no
;                 self type) a `self.f()` is a Method-form call of shape `self`,
;                 bound among the trait's own members.
(call_expression
  function: (field_expression
    value: (self)
    field: (field_identifier) @ref.method.self))

;   @ref.receiver.other — the receiver shape of every method call (S-517, CR-169,
;                 FR-EX-13; the vocabulary is S-514's): `x.f()`, `other.helper()`
;                 inside a method `helper`, `self.field.f()`. It shares its
;                 parent — the `field_expression` — with the call's name node,
;                 which is how the engine pairs them. `self.f()` carries the
;                 `self` mark too (`@ref.method.self` above) and `self` outranks
;                 `other`, so it still reads `self`. An `other` call binds
;                 nowhere: never a same-named free `fn`, never the caller's own
;                 method.
(field_expression
  value: (_) @ref.receiver.other)

; ── Receiver typing (S-587, CR-188, FR-RS-42) ─────────────────────────────────
;
; A call whose receiver's declared type the file proves is retyped to the
; Path-form `T::f`, keeping its `other` shape; the wrappers peeled off the
; declared type (`&`, `&mut`, `Box`, `Arc`, `Rc` — no other) ride on the row.
; The binder binds such a row among T's own methods (S-588). Every capture below is
; a MARKER: it records no row, and the engine pairs each anchor capture with
; the companion captures of its own match (`<anchor>.<role>`).
;
;   @ref.receiver.variable — the receiver of `x.f()`, a plain name.
;   @ref.receiver.self_field — the field of `self.field.f()`; typed by the
;                 caller's own struct (its self type) declaring `field: T`.
(call_expression
  function: (field_expression
    value: (identifier) @ref.receiver.variable))

(call_expression
  function: (field_expression
    value: (field_expression
      value: (self)
      field: (field_identifier) @ref.receiver.self_field)))

;   @ref.receiver.binding — a pattern: every name it binds is one binding of
;                 that name. Its scope is the companion `.scope` node, or the
;                 rest of the `.after` node's parent from where `.after` ends
;                 (a `let`), or — with neither — the whole callable around it
;                 (an over-approximation: a wider scope can only refuse more).
;                 A name is proven only when exactly ONE binding of it is in
;                 scope at the call and that binding carries a proof, so a
;                 shadowed or re-bound name proves nothing.
(let_declaration
  pattern: (_) @ref.receiver.binding) @ref.receiver.binding.after

(function_item
  parameters: (parameters
    (parameter
      pattern: (_) @ref.receiver.binding))) @ref.receiver.binding.scope

(closure_expression
  parameters: (closure_parameters
    (_) @ref.receiver.binding)) @ref.receiver.binding.scope

(for_expression
  pattern: (_) @ref.receiver.binding) @ref.receiver.binding.scope

(match_arm
  pattern: (match_pattern
    .
    (_) @ref.receiver.binding)) @ref.receiver.binding.scope

(let_condition
  pattern: (_) @ref.receiver.binding)

;   @ref.receiver.proof — a binding of one plain name that proves its type:
;                 `.type`, a declared type (a parameter, a typed `let`);
;                 `.constructor`, the callee `T::g` of `let x = T::g(…)`,
;                 proven only when `T` is one segment and every `g` the
;                 caller's module declares on an impl of `T` returns `Self` or
;                 `T`; `.literal`, the type of `let x = T { … }`
;                 when it is one segment (`E::V { … }` builds an `E`).
(parameter
  pattern: (identifier) @ref.receiver.proof
  type: (_) @ref.receiver.proof.type)

(let_declaration
  pattern: (identifier) @ref.receiver.proof
  type: (_) @ref.receiver.proof.type)

(let_declaration
  pattern: (identifier) @ref.receiver.proof
  !type
  value: (call_expression
    function: (scoped_identifier) @ref.receiver.proof.constructor))

(let_declaration
  pattern: (identifier) @ref.receiver.proof
  !type
  value: (struct_expression
    name: (_) @ref.receiver.proof.literal))

;   @ref.receiver.constructor — an associated function's name, with its impl's
;                 `.owner` type and its declared `.returns` type (absent: it
;                 returns `()`).
(impl_item
  type: (_) @ref.receiver.constructor.owner
  body: (declaration_list
    (function_item
      name: (identifier) @ref.receiver.constructor
      return_type: (_)? @ref.receiver.constructor.returns)))

;   @ref.receiver.member — a struct field's name, with its struct's `.owner`
;                 name and its declared `.type`.
(struct_item
  name: (type_identifier) @ref.receiver.member.owner
  body: (field_declaration_list
    (field_declaration
      name: (field_identifier) @ref.receiver.member
      type: (_) @ref.receiver.member.type)))

(use_declaration
  argument: (_) @ref.use)

(macro_invocation) @ref.macro

;   @ref.access — an own-field access (`self.x`): a method reading a field of
;                 its own struct (CR-005, FR-EX-08). The `self` receiver anchors
;                 the capture to an own-member access. Resolution binds it to an
;                 `Accesses` edge (Method → Field) only on an exactly-one Field
;                 candidate in the enclosing container, else it stays unresolved
;                 (NFR-RA-05). The input to the LCOM4 Cohesion dimension.
(field_expression
  value: (self)
  field: (field_identifier) @ref.access)
