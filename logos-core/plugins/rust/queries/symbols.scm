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

; An enum's variant names (S-606, FR-EX-34): the list, so an enum with no
; variant records none, and each variant's name. The enum records them in
; declaration order; a variant is no node, so every symbol is unchanged.
(enum_item
  body: (enum_variant_list) @item.variants)
(enum_variant
  name: (identifier) @item.variant)

(union_item
  name: (type_identifier) @symbol.struct)

(trait_item
  name: (type_identifier) @symbol.trait)

; A trait's required method signature (S-606, CR-202 F3, FR-EX-34) — `fn
; m(&self);`, no body — is a `Method` node and a member of its trait. The
; `@item.signature` marker on the same match says it is a required signature:
; it records `has_body` 0 (`plugin.toml`'s `body_node_kinds`), its range (the
; `@arity.parameters` pattern below) and whether it takes `self`. A signature
; in an `extern` block is no trait member and is not captured. A trait's
; default method (a `function_item`) stays the `@symbol.function` above.
(trait_item
  body: (declaration_list
    (function_signature_item
      name: (identifier) @symbol.method) @item.signature))

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
; A required trait signature's parameter list (S-606), as a function's.
(trait_item
  body: (declaration_list
    (function_signature_item
      parameters: (parameters) @arity.parameters)))

; ── Receiver mode (S-606, CR-202, FR-EX-34) ──────────────────────────────────
; How a callable writes its `self` parameter (`extract::assoc`): by value
; (`self`, `mut self`), `&self`, `&mut self` (a lifetime between them
; included), or typed (`self: Box<Self>`). Every `self_parameter` is captured
; `value` and refined by the more specific patterns; the strongest wins. An
; impl function or required signature whose list writes no receiver records
; the mode `none`; every other callable (a free `fn`, a trait's default body)
; records no mode, as it records no `takes_self`.
(self_parameter) @item.receiver.value
(self_parameter "&") @item.receiver.ref
(self_parameter "&" (mutable_specifier)) @item.receiver.mut
(parameter pattern: (self)) @item.receiver.typed

; ── Impl blocks (S-606, CR-202 F1, FR-EX-34) ─────────────────────────────────
; Every `impl` block — an empty `impl Greet for X {}` included — records its
; header (`extract::assoc`): its self type as written, generics stripped, for
; any shape (a bare, module-relative, `crate::` or external path; `()`, `str`,
; a slice, a tuple); the type a `&T` / `&mut T` self type refers to, flagged
; as a reference; its trait; and an `impl Deref`'s `type Target`. These are
; facts beside the graph, never nodes: every symbol and node is unchanged, and
; the `@symbol.self_type` patterns above keep their narrower rule.
(impl_item
  type: (_) @item.impl.self) @item.impl

(impl_item
  type: (reference_type
    type: (_) @item.impl.referent)) @item.impl

(impl_item
  trait: (_) @item.impl.trait) @item.impl

(impl_item
  trait: (_) @_deref_trait
  body: (declaration_list
    (type_item
      name: (type_identifier) @_target_name
      type: (_) @item.impl.target))
  (#match? @_deref_trait "(^|::)Deref$")
  (#eq? @_target_name "Target")) @item.impl
