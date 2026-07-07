; Go symbol-extraction query (S-015, capability = "symbols").
;
; Capture names map to NodeKind via NodeKind::as_str (extract engine).
; Compiled against the built Go Language at load; fails fast naming this file
; on drift (FR-PL-02). Droppable on disk at
; `.logos/plugins/go/queries/symbols.scm` (FR-PL-04, UAT-PL-03).
;
; v1 policy: struct and interface type specs are captured with their concrete
; kinds; other type declarations (aliases, defined non-struct types) are a
; later increment.

(function_declaration
  name: (identifier) @symbol.function)

(method_declaration
  name: (field_identifier) @symbol.method)

(type_declaration
  (type_spec
    name: (type_identifier) @symbol.struct
    type: (struct_type)))

(type_declaration
  (type_spec
    name: (type_identifier) @symbol.interface
    type: (interface_type)))

(const_declaration
  (const_spec
    name: (identifier) @symbol.constant))

(var_declaration
  (var_spec
    name: (identifier) @symbol.variable))
