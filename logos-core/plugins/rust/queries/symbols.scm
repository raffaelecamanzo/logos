; Rust symbol-extraction query (FR-PL-02, capability = "symbols").
;
; Captures the top-level declarations that become NodeKind nodes in the code
; graph (model::NodeKind). The capture name after the `@` carries the kind so
; the extraction engine (S-007) can map a match to a NodeKind without a second
; lookup. This file is compiled against the built Rust `Language` at load and
; fails fast — naming this path — if a node type or field name drifts.
;
; This query is intentionally droppable-on-disk: placing a modified copy at
; `.logos/plugins/rust/queries/symbols.scm` shadows it without a rebuild
; (FR-PL-04, FR-PL-05, UAT-PL-03).

; v1 policy: every `function_item` is captured as `@symbol.function`, including
; methods defined inside an `impl` block. The extraction engine maps these to
; NodeKind::Function (not Method) because a tree-sitter query cannot express
; "function_item NOT inside impl_item", and associating a method with its
; receiver type is a resolution-engine concern (S-007 / S-011). NodeKind::Method
; is reserved for languages/passes that can bind a method to its type.
(function_item
  name: (identifier) @symbol.function)

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
