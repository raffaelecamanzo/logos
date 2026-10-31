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
; Contains the method via the parent walk. A compact `class A::B` is the class
; node itself, not its scope `A::B` (`extract::lift_to_declaration`), so its
; body is its own.

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

; Every `def` is a Method whatever its name is: an identifier, a constant
; (`def Konst`), a setter (`def x=`, `def self.x=`) or an operator (`def []`,
; `def <=>`, `def -@`) — S-564, CR-179, FR-EX-26. The grammar also lets a
; method name be a symbol or a variable (`def :s`, `def @x`, `def $g`): none of
; those is a method, and error recovery produces them from torn source.
(method
  name: [(identifier) (constant) (setter) (operator)] @symbol.method)

(singleton_method
  name: [(identifier) (constant) (setter) (operator)] @symbol.method)

; ── Instance state (S-564, CR-179, FR-EX-26) ─────────────────────────────────
; `@symbol.state` marks a declaration that is state of the class or module
; around it, whatever method or block it sits in (`extract::settle_state`): it
; is captured on its name alone, owns no scope and no reference, is a Field of
; the nearest enclosing class or module, and is declared once per name there —
; at its first occurrence. An `attr_*` symbol or string names `@x`, as the first
; assignment of `@x` does, so the two are one Field. State outside a class or
; module is no Field.
; `attr_reader`, `attr_writer` and `attr_accessor` with no receiver: each symbol
; or plain string of the list.
(call
  !receiver
  method: (identifier) @_attr
  arguments: (argument_list
    [(simple_symbol) @symbol.field @symbol.state
     (string . (string_content) @symbol.field @symbol.state .)])
  (#any-of? @_attr "attr_reader" "attr_writer" "attr_accessor"))
; An `@x = …` or `@x ||= …` assignment, and each `@x` of `@a, @b = …`.
(assignment
  left: (instance_variable) @symbol.field @symbol.state)
(operator_assignment
  left: (instance_variable) @symbol.field @symbol.state)
(assignment
  left: (left_assignment_list
    (instance_variable) @symbol.field @symbol.state))

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

; ── Callables of the class or module object (S-565, CR-179, FR-RS-33) ────────
; The `@item.receiver.none` marker (`extract::assoc`) records a method that is
; called on its class or module object itself, not on an instance — what a call
; on a constant (`Foo.bar`) binds among. It names no declaration of its own.
; `def self.x`:
(singleton_method object: (self) @item.receiver.none)
; a method of a `class << self` body:
(singleton_class
  value: (self)
  body: (body_statement
    (method name: (_) @item.receiver.none)))
; and every method a module defines after a bare `module_function` — directly,
; or inside a conditional (`def x … end unless c`, `if … else def x … end`), as
; rack's `Utils.escape_html` is; never a method of a class or module nested in
; the module, whose instances own it. `module_function :name` names its methods
; by symbol, which a query cannot pair with a definition, and `extend self` is
; not read: such methods stay instance methods here.
(module
  body: (body_statement
    (identifier) @_module_function
    [(method name: (_) @item.receiver.none)
     (if_modifier (method name: (_) @item.receiver.none))
     (unless_modifier (method name: (_) @item.receiver.none))
     (begin (method name: (_) @item.receiver.none))
     (if (then (method name: (_) @item.receiver.none)))
     (if (else (method name: (_) @item.receiver.none)))
     (unless (then (method name: (_) @item.receiver.none)))
     (unless (else (method name: (_) @item.receiver.none)))])
  (#eq? @_module_function "module_function"))
; A bare `private`, `public` or `protected` after `module_function` ends it: a
; method defined after both is an instance method again. Its `value` receiver
; outranks the `none` above (`extract::assoc`), so it records no singleton.
(module
  body: (body_statement
    (identifier) @_module_function
    (identifier) @_visibility
    [(method name: (_) @item.receiver.value)
     (if_modifier (method name: (_) @item.receiver.value))
     (unless_modifier (method name: (_) @item.receiver.value))
     (begin (method name: (_) @item.receiver.value))
     (if (then (method name: (_) @item.receiver.value)))
     (if (else (method name: (_) @item.receiver.value)))
     (unless (then (method name: (_) @item.receiver.value)))
     (unless (else (method name: (_) @item.receiver.value)))])
  (#eq? @_module_function "module_function")
  (#any-of? @_visibility "private" "public" "protected"))
; A method defined in a block that is a statement of a class or module body
; (S-568, FR-RS-35) — a Rails concern's `class_methods do def x … end end` — is
; defined on whatever object the block is evaluated against, which the syntax
; does not say: it is never one of the body's instance methods, so an instance
; call walking the ancestors never reaches it (metaprogramming, CR-179 §3.3).
(class
  body: (body_statement
    (call
      block: [(do_block body: (body_statement (method name: (_) @item.receiver.none)))
              (block body: (block_body (method name: (_) @item.receiver.none)))])))
(module
  body: (body_statement
    (call
      block: [(do_block body: (body_statement (method name: (_) @item.receiver.none)))
              (block body: (block_body (method name: (_) @item.receiver.none)))])))
