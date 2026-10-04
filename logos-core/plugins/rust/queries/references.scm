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
;                 knowable without type inference. It carries no receiver
;                 marker, so it records no shape and binds nowhere under any
;                 policy (S-514, FR-RS-12).
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
;                 `@ref.method.self` records (S-514).
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
