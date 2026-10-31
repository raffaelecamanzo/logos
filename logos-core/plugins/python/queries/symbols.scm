; Python symbol-extraction query (S-015, capability = "symbols").
;
; Captures the declarations that become NodeKind nodes. The capture name after
; the `@` carries the kind (extract::kind_for_capture maps it via
; NodeKind::as_str). Compiled against the built Python Language on its first use, or at
; load as an on-disk override; drift fails naming this file (FR-PL-02,
; CR-197). Droppable on disk at
; `.logos/plugins/python/queries/symbols.scm` (FR-PL-04, UAT-PL-03).
;
; A `def` → Function, unless it is written in a class body (the `@decl.method`
; markers below). Every def is named by this one pattern, and a method is
; re-kinded by its marker (extract::definition): a second pattern naming a
; method would name the same declaration twice, the first taken whichever the
; query lists first. Function and Method render one descriptor, so the symbol is
; the same either way. The enclosing class Contains it via the parent walk.

(function_definition
  name: (identifier) @symbol.function)

(class_definition
  name: (identifier) @symbol.class)

; A def of a class body → Method (S-543, CR-175, FR-EX-20): written directly in
; the body, or decorated there (`@staticmethod`, `@classmethod`, `@property`
; and any other decorator), and one level down an `if`/`elif`/`else` or a
; `try`/`except`/`else`/`finally` of the body (`if TYPE_CHECKING:`, a version
; guard). A def deeper than that, or under another compound statement, stays a
; Function; so does a def in a def, a local and not a member.
(class_definition
  body: (block
    [(function_definition) @decl.method
     (decorated_definition definition: (function_definition) @decl.method)]))
(class_definition
  body: (block
    (if_statement
      consequence: (block
        [(function_definition) @decl.method
         (decorated_definition definition: (function_definition) @decl.method)]))))
(class_definition
  body: (block
    (if_statement
      alternative: (elif_clause
        consequence: (block
          [(function_definition) @decl.method
           (decorated_definition definition: (function_definition) @decl.method)])))))
(class_definition
  body: (block
    (if_statement
      alternative: (else_clause
        body: (block
          [(function_definition) @decl.method
           (decorated_definition definition: (function_definition) @decl.method)])))))
(class_definition
  body: (block
    (try_statement
      body: (block
        [(function_definition) @decl.method
         (decorated_definition definition: (function_definition) @decl.method)]))))
(class_definition
  body: (block
    (try_statement
      [(except_clause
         (block
           [(function_definition) @decl.method
            (decorated_definition definition: (function_definition) @decl.method)]))
       (else_clause
         body: (block
           [(function_definition) @decl.method
            (decorated_definition definition: (function_definition) @decl.method)]))
       (finally_clause
         (block
           [(function_definition) @decl.method
            (decorated_definition definition: (function_definition) @decl.method)]))])))

; ── Class attributes (S-543, CR-175, FR-EX-20) ───────────────────────────────
; A class's Fields: the assignments of its body — a plain `x = …`, an annotated
; `x: int = …` and a bare annotation `x: int`, which is how a dataclass, attrs,
; Django or pydantic model declares its fields — and its methods' `self.x = …`
; writes, annotated or not. `@symbol.attribute` marks each an attribute an
; assignment declares (`extract::ATTRIBUTE_CAPTURE`): taken at its name, so the
; assignment's initialiser calls stay the class's and a method's `self.x` write
; stays that method's access; owned by the class, the write's method's class;
; and declared once per class, a class-body one ahead of the first write. Each
; name of a tuple target is one (`a, b = …`, `self.a, self.b = …`). A
; `self.x = …` outside a method of a class declares nothing. Not captured: the
; names after the first of a chained assignment (`a = b = 0`), augmented
; assignments (`self.n += 1`), `cls.x = …`, and assignments under a compound
; statement of the body.
(class_definition
  body: (block
    (expression_statement
      (assignment
        left: [(identifier) @symbol.field @symbol.attribute
               (pattern_list (identifier) @symbol.field @symbol.attribute)
               (tuple_pattern (identifier) @symbol.field @symbol.attribute)]))))
((assignment
  left: [(attribute
           object: (identifier) @_self
           attribute: (identifier) @symbol.field @symbol.attribute)
         (pattern_list
           (attribute
             object: (identifier) @_self
             attribute: (identifier) @symbol.field @symbol.attribute))
         (tuple_pattern
           (attribute
             object: (identifier) @_self
             attribute: (identifier) @symbol.field @symbol.attribute))])
  (#eq? @_self "self"))

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
