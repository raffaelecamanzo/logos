; Python symbol-extraction query (S-015, capability = "symbols").
;
; Captures the declarations that become NodeKind nodes. The capture name after
; the `@` carries the kind (extract::kind_for_capture maps it via
; NodeKind::as_str). Compiled against the built Python Language on its first use, or at
; load as an on-disk override; drift fails naming this file (FR-PL-02,
; CR-197). Droppable on disk at
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

; ── Parameter range (S-591, CR-190, FR-EX-32) ────────────────────────────────
; The `@arity.*` vocabulary the extraction engine reads (`extract::arity`): a
; callable's parameter list, and each parameter as required, optional (a
; default value raises only the maximum), variadic (an unbounded maximum), a
; receiver (not counted) or no parameter at all (skip). A list child no capture
; covers, or one captured `@arity.unknown`, records the range unknown rather
; than miscounting it. Captures never name a declaration, so every symbol and
; node is unchanged.
; A parameter with a default is optional; `*args` and `**kwargs` are variadic;
; the bare `*` and `/` separators are no parameter. A method's first parameter
; (`self`, `cls`) is its receiver — unless it is a `@staticmethod`.
(function_definition
  parameters: (parameters) @arity.parameters)
(parameters (identifier) @arity.required)
(parameters (typed_parameter) @arity.required)
(parameters (default_parameter) @arity.optional)
(parameters (typed_default_parameter) @arity.optional)
(parameters (list_splat_pattern) @arity.variadic)
(parameters (dictionary_splat_pattern) @arity.variadic)
(parameters
  (typed_parameter [(list_splat_pattern) (dictionary_splat_pattern)]) @arity.variadic)
(parameters (keyword_separator) @arity.skip)
(parameters (positional_separator) @arity.skip)
(class_definition
  body: (block
    (function_definition
      parameters: (parameters . [(identifier) (typed_parameter)] @arity.receiver))))
(class_definition
  body: (block
    (decorated_definition
      (decorator (identifier) @_decorator)*
      definition: (function_definition
        parameters: (parameters . [(identifier) (typed_parameter)] @arity.receiver))
      (#not-eq? @_decorator "staticmethod"))))
; The same receiver behind a comment opening the list — `def update(  # type:
; ignore[override]` puts the comment first, which the anchor would otherwise
; stop at, so `self` was counted (S-592).
(class_definition
  body: (block
    (function_definition
      parameters: (parameters . (comment) . [(identifier) (typed_parameter)] @arity.receiver))))
(class_definition
  body: (block
    (decorated_definition
      (decorator (identifier) @_decorator)*
      definition: (function_definition
        parameters: (parameters . (comment) . [(identifier) (typed_parameter)] @arity.receiver))
      (#not-eq? @_decorator "staticmethod"))))
; A method defined under a compound statement of a class body (`if`, `try`,
; `with`, …) takes `self` too, but a query cannot see whether the compound
; statement sits in a class body or anywhere else, so a function defined
; directly in a compound statement's block records an unknown range rather than
; counting a receiver. One pattern per statement kind: an alternation cannot
; stand as a parent.
(if_statement
  (block [(function_definition parameters: (parameters) @arity.unknown)
          (decorated_definition definition: (function_definition parameters: (parameters) @arity.unknown))]))
(elif_clause
  (block [(function_definition parameters: (parameters) @arity.unknown)
          (decorated_definition definition: (function_definition parameters: (parameters) @arity.unknown))]))
(else_clause
  (block [(function_definition parameters: (parameters) @arity.unknown)
          (decorated_definition definition: (function_definition parameters: (parameters) @arity.unknown))]))
(try_statement
  (block [(function_definition parameters: (parameters) @arity.unknown)
          (decorated_definition definition: (function_definition parameters: (parameters) @arity.unknown))]))
(except_clause
  (block [(function_definition parameters: (parameters) @arity.unknown)
          (decorated_definition definition: (function_definition parameters: (parameters) @arity.unknown))]))
(finally_clause
  (block [(function_definition parameters: (parameters) @arity.unknown)
          (decorated_definition definition: (function_definition parameters: (parameters) @arity.unknown))]))
(with_statement
  (block [(function_definition parameters: (parameters) @arity.unknown)
          (decorated_definition definition: (function_definition parameters: (parameters) @arity.unknown))]))
(for_statement
  (block [(function_definition parameters: (parameters) @arity.unknown)
          (decorated_definition definition: (function_definition parameters: (parameters) @arity.unknown))]))
(while_statement
  (block [(function_definition parameters: (parameters) @arity.unknown)
          (decorated_definition definition: (function_definition parameters: (parameters) @arity.unknown))]))
(case_clause
  (block [(function_definition parameters: (parameters) @arity.unknown)
          (decorated_definition definition: (function_definition parameters: (parameters) @arity.unknown))]))
