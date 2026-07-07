; Ruby symbol-extraction query (S-059, capability = "symbols").
;
; Captures the declarations that become NodeKind nodes. The capture name after
; the `@` carries the kind (extract::kind_for_capture maps it via
; NodeKind::as_str). Compiled against the built Ruby Language at load; fails
; fast naming this file on drift (FR-PL-02). Droppable on disk at
; `.logos/plugins/ruby/queries/symbols.scm` (FR-PL-04, UAT-PL-03).
;
; Class capture is the class-bearing applicability declaration (CR-009,
; FR-QM-11): mapping `class` to NodeKind::Class is what makes Cohesion/LCOM4
; and Focus applicable to Ruby — "the kind a construct extracts to is its
; declared applicability" (metrics::extended). A Ruby `module` is a namespace,
; mapped to NodeKind::Module (not class-applicable, the honest answer).
;
; v1 policy (mirrors the other grammars'): every `method`/`singleton_method` —
; including a method inside a class body — maps to NodeKind::Method; binding a
; method to its receiver type is a resolution concern. The enclosing class still
; Contains the method via the parent walk.

(class
  name: (constant) @symbol.class)

(class
  name: (scope_resolution
    name: (constant) @symbol.class))

(module
  name: (constant) @symbol.module)

(module
  name: (scope_resolution
    name: (constant) @symbol.module))

(method
  name: (identifier) @symbol.method)

(singleton_method
  name: (identifier) @symbol.method)
