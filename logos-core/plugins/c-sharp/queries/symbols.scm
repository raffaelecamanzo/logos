; C# symbol-extraction query (S-057, CR-009, capability = "symbols").
;
; Capture names map to NodeKind via NodeKind::as_str (extract engine).
; Compiled against the built C# Language on its first use, or at
; load as an on-disk override; drift fails naming this file (FR-PL-02,
; CR-197). Droppable on disk at
; `.logos/plugins/c-sharp/queries/symbols.scm` (FR-PL-04, UAT-PL-03).
;
; v1 policy (matching Java): constructors are deliberately NOT captured — a
; constructor shares its type's name, and a second same-named node would make
; every `TypeName` reference ambiguous under the binder's exactly-one-or-nothing
; rule (NFR-RA-05). `new TypeName()` references stay honestly unresolved.

(class_declaration
  name: (identifier) @symbol.class)

; Records are reference types declared like classes (`record R(...)` / `record R {}`).
(record_declaration
  name: (identifier) @symbol.class)

(interface_declaration
  name: (identifier) @symbol.interface)

(struct_declaration
  name: (identifier) @symbol.struct)

(enum_declaration
  name: (identifier) @symbol.enum)

(method_declaration
  name: (identifier) @symbol.method)

; A property is a field-like member (the LCOM4/Cohesion structural input,
; CR-005) — captured as a Field, the closest v1 NodeKind.
(property_declaration
  name: (identifier) @symbol.field)

(field_declaration
  (variable_declaration
    (variable_declarator
      name: (identifier) @symbol.field)))

; The file's namespace (S-518, CR-170, FR-RS-13). Not a declaration — its
; capture group is `module`, not `symbol`, so the declaration walk skips it. A
; file-scoped `namespace A.B;` and a block `namespace A.B { … }` give the same
; identity, and nested blocks (`namespace A { namespace B { … } }`) compose to
; `A.B`; each top-level type is named that namespace plus its own name, whatever
; directory the file sits in.
(namespace_declaration
  name: (_) @module.namespace)

(file_scoped_namespace_declaration
  name: (_) @module.namespace)

; ── Parameter range (S-591, CR-190, FR-EX-32) ────────────────────────────────
; The `@arity.*` vocabulary the extraction engine reads (`extract::arity`): a
; callable's parameter list, and each parameter as required, optional (a
; default value raises only the maximum), variadic (an unbounded maximum), a
; receiver (not counted) or no parameter at all (skip). A list child no capture
; covers, or one captured `@arity.unknown`, records the range unknown rather
; than miscounting it. Captures never name a declaration, so every symbol and
; node is unchanged.
; A parameter with `= value` is optional. A `params` array is written on the
; list itself — the `params` token, then its type and name — and is variadic.
; An extension method's `this` parameter is its receiver at `x.M()` and an
; argument at `C.M(x)`, so its range is unknown.
(method_declaration
  parameters: (parameter_list) @arity.parameters)
(parameter_list (parameter) @arity.required)
(parameter_list (parameter "=") @arity.optional)
(parameter_list "params" @arity.variadic)
(parameter_list type: (_) @arity.skip)
(parameter_list name: (_) @arity.skip)
(parameter_list
  (parameter (modifier) @_this) @arity.unknown
  (#eq? @_this "this"))
