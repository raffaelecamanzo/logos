; TSX symbol-extraction query (S-015, capability = "symbols").
;
; Capture names map to NodeKind via NodeKind::as_str (extract engine).
; Compiled against the built TSX Language at load; fails fast naming
; this file on drift (FR-PL-02). Droppable on disk at
; `.logos/plugins/tsx/queries/symbols.scm` (FR-PL-04, UAT-PL-03).
;
; v1 policy: arrow/function expressions bound to a `const`/`let` name are
; captured as functions (the dominant JS declaration style); plain variable
; declarations are not captured (a later increment).

(function_declaration
  name: (identifier) @symbol.function)

(class_declaration
  name: (type_identifier) @symbol.class)

(interface_declaration
  name: (type_identifier) @symbol.interface)

(enum_declaration
  name: (identifier) @symbol.enum)

(type_alias_declaration
  name: (type_identifier) @symbol.type_alias)

(method_definition
  name: (property_identifier) @symbol.method)

(variable_declarator
  name: (identifier) @symbol.function
  value: (arrow_function))

(variable_declarator
  name: (identifier) @symbol.function
  value: (function_expression))
