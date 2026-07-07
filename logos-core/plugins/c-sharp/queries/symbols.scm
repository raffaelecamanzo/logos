; C# symbol-extraction query (S-057, CR-009, capability = "symbols").
;
; Capture names map to NodeKind via NodeKind::as_str (extract engine).
; Compiled against the built C# Language at load; fails fast naming this
; file on drift (FR-PL-02). Droppable on disk at
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
