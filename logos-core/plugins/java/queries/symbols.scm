; Java symbol-extraction query (S-015, capability = "symbols").
;
; Capture names map to NodeKind via NodeKind::as_str (extract engine).
; Compiled against the built Java Language at load; fails fast naming this
; file on drift (FR-PL-02). Droppable on disk at
; `.logos/plugins/java/queries/symbols.scm` (FR-PL-04, UAT-PL-03).
;
; v1 policy: constructors are deliberately NOT captured — a constructor
; shares its class's name, and a second same-named node would make every
; `ClassName` reference ambiguous under the binder's exactly-one-or-nothing
; rule (NFR-RA-05). `new ClassName()` references stay honestly unresolved.

(class_declaration
  name: (identifier) @symbol.class)

(record_declaration
  name: (identifier) @symbol.class)

(interface_declaration
  name: (identifier) @symbol.interface)

(enum_declaration
  name: (identifier) @symbol.enum)

(method_declaration
  name: (identifier) @symbol.method)

(field_declaration
  declarator: (variable_declarator
    name: (identifier) @symbol.field))
