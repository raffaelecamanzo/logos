; TypeScript symbol-extraction query (S-015, capability = "symbols").
;
; Capture names map to NodeKind via NodeKind::as_str (extract engine).
; Compiled against the built TypeScript Language at load; fails fast naming
; this file on drift (FR-PL-02). Droppable on disk at
; `.logos/plugins/typescript/queries/symbols.scm` (FR-PL-04, UAT-PL-03).
;
; v1 policy: arrow/function expressions bound to a `const`/`let` name are
; captured as functions (the dominant JS declaration style); plain variable
; declarations are not captured (a later increment).
;
; Class fields are `@symbol.field` (S-477, CR-154, FR-EX-08): a declared field
; (`count = 0`), a `#private` one (`#secret`, whose name is a
; `private_property_identifier` — the name keeps its `#`, exactly as `this.#secret`
; spells it), and a constructor parameter property (`private readonly http`, marked
; by an accessibility / `override` / `readonly` modifier — a bare parameter is not
; a field). They are what the `this.x` capture in references.scm binds to. A
; field initialised with an arrow function is a field, not a function.

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

(public_field_definition
  name: (property_identifier) @symbol.field)

(public_field_definition
  name: (private_property_identifier) @symbol.field)

(required_parameter
  [(accessibility_modifier) (override_modifier) "readonly"]
  pattern: (identifier) @symbol.field)

(optional_parameter
  [(accessibility_modifier) (override_modifier) "readonly"]
  pattern: (identifier) @symbol.field)
