; PHP symbol-extraction query (S-060, capability = "symbols").
;
; Capture names map to NodeKind via NodeKind::as_str (extract engine).
; Compiled against the built PHP Language at load; fails fast naming this
; file on drift (FR-PL-02). Droppable on disk at
; `.logos/plugins/php/queries/symbols.scm` (FR-PL-04, UAT-PL-03).
;
; Classes map to NodeKind::Class, which is what makes Cohesion (LCOM4)
; applicable for PHP (ADR-21 declarative applicability: the kind a construct
; extracts to *is* its declared applicability). Interfaces/traits/enums map to
; their own kinds and are correctly excluded from cohesion scope.
;
; v1 policy: `__construct` is captured like any other method — its name never
; collides with the class name (unlike a Java constructor), so a `new Class()`
; reference stays an honest unresolved class reference, not an ambiguous one.

(class_declaration
  name: (name) @symbol.class)

(interface_declaration
  name: (name) @symbol.interface)

(trait_declaration
  name: (name) @symbol.trait)

(enum_declaration
  name: (name) @symbol.enum)

(method_declaration
  name: (name) @symbol.method)

(function_definition
  name: (name) @symbol.function)

; A typed-or-untyped class property: `public int $balance = 0;`. The captured
; name is the inner `name` of the `$variable` (`balance`), so a `$this->balance`
; own-field access (references.scm) binds to it for LCOM4 (FR-EX-08).
(property_declaration
  (property_element
    name: (variable_name (name) @symbol.field)))
