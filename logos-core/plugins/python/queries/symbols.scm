; Python symbol-extraction query (S-015, capability = "symbols").
;
; Captures the declarations that become NodeKind nodes. The capture name after
; the `@` carries the kind (extract::kind_for_capture maps it via
; NodeKind::as_str). Compiled against the built Python Language at load;
; fails fast naming this file on drift (FR-PL-02). Droppable on disk at
; `.logos/plugins/python/queries/symbols.scm` (FR-PL-04, UAT-PL-03).
;
; v1 policy (mirrors the Rust grammar's): every `function_definition` —
; including methods inside a class body — maps to NodeKind::Function, because
; a query cannot express "def NOT inside class", and binding a method to its
; receiver type is a resolution concern. The enclosing class still Contains
; the function via the parent walk.

(function_definition
  name: (identifier) @symbol.function)

(class_definition
  name: (identifier) @symbol.class)
