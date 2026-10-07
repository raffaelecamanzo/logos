; Ruby symbol-extraction query (S-059, capability = "symbols").
;
; Captures the declarations that become NodeKind nodes. The capture name after
; the `@` carries the kind (extract::kind_for_capture maps it via
; NodeKind::as_str). Compiled against the built Ruby Language on its first use, or at
; load as an on-disk override; drift fails naming this file (FR-PL-02,
; CR-197). Droppable on disk at
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

; ── Parameter range (S-591, CR-190, FR-EX-32) ────────────────────────────────
; The `@arity.*` vocabulary the extraction engine reads (`extract::arity`): a
; callable's parameter list, and each parameter as required, optional (a
; default value raises only the maximum), variadic (an unbounded maximum), a
; receiver (not counted) or no parameter at all (skip). A list child no capture
; covers, or one captured `@arity.unknown`, records the range unknown rather
; than miscounting it. Captures never name a declaration, so every symbol and
; node is unchanged.
; A parameter with a default is optional, a keyword without one required;
; `*args`, `**opts` and `...` are variadic; a `&block` is no parameter.
(method
  parameters: (method_parameters) @arity.parameters)
(singleton_method
  parameters: (method_parameters) @arity.parameters)
(method_parameters (identifier) @arity.required)
(method_parameters (destructured_parameter) @arity.required)
(method_parameters (optional_parameter) @arity.optional)
(method_parameters (keyword_parameter) @arity.required)
(method_parameters (keyword_parameter value: (_)) @arity.optional)
(method_parameters (splat_parameter) @arity.variadic)
(method_parameters (hash_splat_parameter) @arity.variadic)
(method_parameters (forward_parameter) @arity.variadic)
(method_parameters (block_parameter) @arity.skip)
