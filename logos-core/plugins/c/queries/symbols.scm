; C symbol-extraction query (S-056, capability = "symbols").
;
; Capture names map to NodeKind via NodeKind::as_str (extract engine). Compiled
; against the built C Language at load; fails fast naming this file on drift
; (FR-PL-02). Droppable on disk at `.logos/plugins/c/queries/symbols.scm`
; (FR-PL-04, UAT-PL-03).
;
; Honesty posture (NFR-CC-04): C is NOT class-applicable. Its `struct`/`union`
; aggregates are deliberately NOT captured as `@symbol.struct` — a C struct has
; fields but no methods, so mapping it to NodeKind::Struct would make the Focus
; dimension (which scopes over Class ∪ Struct containers) score a method-less
; aggregate as a perfect 1.0: a fabricated clean score. Emitting neither Class
; nor Struct keeps Cohesion (Class) and Focus (Class ∪ Struct) honestly `n/a`
; for C (FR-QM-11, FR-CV-08 — n/a paths).

; Function *definitions* only — a bare prototype is not the symbol, its
; definition is. The name's `function_declarator` is nested under the
; `declarator` field of the definition, possibly through one or two
; `pointer_declarator`s for pointer-returning functions (`int *f()`,
; `char **g()`). All three return shapes are anchored to the definition's
; `declarator` field, so a function-pointer *parameter* (which lives under the
; `parameters` field) is never mistaken for a declaration. The extract engine
; then ascends the `declarator`-field chain to the `function_definition`, so
; metrics, span, and the body scope a call resolves into see the whole
; definition.
(function_definition
  declarator: [
    (function_declarator
      declarator: (identifier) @symbol.function)
    (pointer_declarator
      declarator: (function_declarator
        declarator: (identifier) @symbol.function))
    (pointer_declarator
      declarator: (pointer_declarator
        declarator: (function_declarator
          declarator: (identifier) @symbol.function)))])

; `typedef NAME` — a type alias (a name, not a class-like container).
(type_definition
  declarator: (type_identifier) @symbol.type_alias)

; A named `enum`. NodeKind::Enum is outside the Class ∪ Struct container set, so
; capturing it never perturbs the Cohesion/Focus applicability posture.
(enum_specifier
  name: (type_identifier) @symbol.enum)

; Enumerators — file-scope named compile-time constants.
(enumerator
  name: (identifier) @symbol.constant)

; Object-style and function-style preprocessor macros.
(preproc_def
  name: (identifier) @symbol.macro)
(preproc_function_def
  name: (identifier) @symbol.macro)

; File-scope object declarations (globals). Anchored to `translation_unit` so a
; local variable inside a function body is never mistaken for a file-scope
; symbol. Covers the bare (`int y;`), initialised (`int x = 0;`), and pointer
; (`char *s;`, `int *p = 0;`) shapes.
(translation_unit
  (declaration
    declarator: [
      (identifier) @symbol.variable
      (pointer_declarator
        declarator: (identifier) @symbol.variable)
      (init_declarator
        declarator: (identifier) @symbol.variable)
      (init_declarator
        declarator: (pointer_declarator
          declarator: (identifier) @symbol.variable))]))
