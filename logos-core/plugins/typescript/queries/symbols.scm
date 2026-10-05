; TypeScript symbol-extraction query (S-015, capability = "symbols").
;
; Capture names map to NodeKind via NodeKind::as_str (extract engine).
; Compiled against the built TypeScript Language on its first use, or at
; load as an on-disk override; drift fails naming this file (FR-PL-02,
; CR-197). Droppable on disk at
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
;
; Both patterns are anchored to the body of a `class_declaration`, the only class
; form the symbols above capture as `@symbol.class`. A field of an abstract class or
; a class expression would otherwise be a file-scope `Field` — no class to own it,
; so nothing could bind it, yet it would join every bare-name candidate set — and a
; modifier-bearing parameter of an interface method, a function type or a function
; (none of them a constructor) would become a bogus field.

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

(class_declaration
  body: (class_body
    (public_field_definition
      name: [(property_identifier) (private_property_identifier)] @symbol.field)))

(class_declaration
  body: (class_body
    (method_definition
      parameters: (formal_parameters
        [(required_parameter
           [(accessibility_modifier) (override_modifier) "readonly"]
           pattern: (identifier) @symbol.field)
         (optional_parameter
           [(accessibility_modifier) (override_modifier) "readonly"]
           pattern: (identifier) @symbol.field)]))))
