; Rust symbol-extraction query (FR-PL-02, capability = "symbols").
;
; Captures the top-level declarations that become NodeKind nodes in the code
; graph (model::NodeKind). The capture name after the `@` carries the kind so
; the extraction engine (S-007) can map a match to a NodeKind without a second
; lookup. This file is compiled against the built Rust `Language` on its first
; use, or at load as an on-disk override, and fails — naming this path — if a
; node type or field name drifts (CR-197).
;
; This query is intentionally droppable-on-disk: placing a modified copy at
; `.logos/plugins/rust/queries/symbols.scm` shadows it without a rebuild
; (FR-PL-04, FR-PL-05, UAT-PL-03).

; v1 policy: every `function_item` is captured as `@symbol.function`, including
; methods defined inside an `impl` block. The extraction engine maps these to
; NodeKind::Function because a tree-sitter query cannot express "function_item
; NOT inside impl_item"; it re-kinds an impl method as Method at emission
; (CR-068 Part B). The impl's self type rides beside the method's node, never in
; its symbol — the `@symbol.self_type` patterns after this one (S-493).
(function_item
  name: (identifier) @symbol.function)

; A method's self type (S-493, FR-RS-11): an `impl` block's directly nested
; `function_item` is captured once more, with `@symbol.self_type` on the base
; type name of the block's self type — the last path segment, generics outside
; the capture: `impl<M> A<M>` → `A`, `impl fmt::Display for crate::x::Y` → `Y`,
; `impl<T> super::B<T>` → `B`. The extraction engine gives the self type to the
; declaration the same match captures (and takes the declaration once, whichever
; pattern names it first), so symbols and kinds are unchanged; it is persisted
; beside the node (`nodes.self_type`) for the binder's `self.m()` / `Self::m()`
; binding.
;
; A path-qualified header records a self type only when the path is the
; crate's own — headed `crate`, `self` or `super`. `impl Ext for
; std::io::Error` is a foreign type's impl, and keeping its base name `Error`
; would let it pass for a crate type of that name; it records none, and its
; methods' self calls resolve as before. A self type of any other shape — `&T`,
; `[T]`, `(A, B)`, `dyn T` — records none either. (A bare name the file imports
; from outside the crate is refused by the binder, which reads the imports.)
(impl_item
  type: (type_identifier) @symbol.self_type
  body: (declaration_list
    (function_item
      name: (identifier) @symbol.function)))

(impl_item
  type: (generic_type
    type: (type_identifier) @symbol.self_type)
  body: (declaration_list
    (function_item
      name: (identifier) @symbol.function)))

(impl_item
  type: (scoped_type_identifier
    path: (_) @_self_type_path
    name: (type_identifier) @symbol.self_type)
  body: (declaration_list
    (function_item
      name: (identifier) @symbol.function))
  (#match? @_self_type_path "^(crate|self|super)($|::)"))

(impl_item
  type: (generic_type
    type: (scoped_type_identifier
      path: (_) @_self_type_path
      name: (type_identifier) @symbol.self_type))
  body: (declaration_list
    (function_item
      name: (identifier) @symbol.function))
  (#match? @_self_type_path "^(crate|self|super)($|::)"))

(struct_item
  name: (type_identifier) @symbol.struct)

(enum_item
  name: (type_identifier) @symbol.enum)

(union_item
  name: (type_identifier) @symbol.struct)

(trait_item
  name: (type_identifier) @symbol.trait)

(mod_item
  name: (identifier) @symbol.module)

(const_item
  name: (identifier) @symbol.constant)

(static_item
  name: (identifier) @symbol.variable)

(type_item
  name: (type_identifier) @symbol.type_alias)

(macro_definition
  name: (identifier) @symbol.macro)

; ── Parameter range (S-591, CR-190, FR-EX-32) ────────────────────────────────
; The `@arity.*` vocabulary the extraction engine reads (`extract::arity`): a
; callable's parameter list, and each parameter as required, optional (a
; default value raises only the maximum), variadic (an unbounded maximum), a
; receiver (not counted) or no parameter at all (skip). A list child no capture
; covers, or one captured `@arity.unknown`, records the range unknown rather
; than miscounting it. Captures never name a declaration, so every symbol and
; node is unchanged.
; A `self` receiver — `self`, `mut self`, `&self`, `&'a mut self`, or a `self`
; pattern with an explicit type (`self: Box<Self>`, `self: Pin<&mut Self>`) — is
; not counted, and an impl function writing one takes `self` (CR-200); an extern
; `...` is variadic; an attribute is no parameter.
(function_item
  parameters: (parameters) @arity.parameters)
(parameters (parameter) @arity.required)
(parameters (self_parameter) @arity.receiver)
(parameters (parameter pattern: (self)) @arity.receiver)
(parameters (variadic_parameter) @arity.variadic)
(parameters (attribute_item) @arity.skip)
