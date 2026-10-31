; TSX symbol-extraction query (S-015, capability = "symbols").
;
; Capture names map to NodeKind via NodeKind::as_str (extract engine).
; Compiled against the built TSX Language on its first use, or at
; load as an on-disk override; drift fails naming this file (FR-PL-02,
; CR-197). Droppable on disk at
; `.logos/plugins/tsx/queries/symbols.scm` (FR-PL-04, UAT-PL-03).
;
; Arrow/function expressions bound to a `const`/`let` name are captured as
; functions (the dominant JS declaration style). The declarations S-532 adds
; (CR-173, FR-EX-16) are each a new symbol, and none moves a symbol the graph held
; except the methods of an abstract class, which sit under it now:
;
; - An `abstract class` is a Class, and its `abstract` method signatures are
;   bodyless Methods marked `@item.signature` (a required signature its
;   subclasses supply, so a call never binds to it).
; - An interface's method members are bodyless Methods marked the same way. A
;   property, call, construct or index signature names no method and is not
;   captured, nor is a type literal's member, nor a class's overload signature
;   (the class's one callable is its implementation).
; - A PascalCase `const X = forwardRef(…)` / `memo(…)` / `React.forwardRef(…)` /
;   `React.memo(…)`, or a `styled.<tag>` / `styled(X)` tagged template, is a
;   component: a Function. The wrapper call is its body (`body_node_kinds`). It
;   owns the references of its body but no scope, so a handler declared inside
;   keeps its symbol. A typed `styled.div<P>` tag and an `.attrs(…)` call parse as
;   other expressions and are Constants.
; - Any other top-level `const` is a Constant, a top-level `let`/`var` a
;   Variable. A binding with a function value is already a Function above: a
;   declaration both patterns name is taken as the Function whichever matched
;   first (`collect_decls`), so a Constant/Variable pattern never has to
;   exclude it. A destructuring binding and a nested one are not captured, and a
;   Constant/Variable owns neither scope nor references (`is_value_binding`).
; - `export default <expression>` is a Constant named `default`, captured on
;   its `default` keyword, at file level only; `export default function f` /
;   `class C` keep their own names (they carry a `declaration:`, not a `value:`).
;   A decorated abstract member is not captured: the grammar does not parse one.
;
; Class fields are `@symbol.field` (S-477, CR-154, FR-EX-08): a declared field
; (`count = 0`), a `#private` one (`#secret`, whose name is a
; `private_property_identifier` — the name keeps its `#`, exactly as `this.#secret`
; spells it), and a constructor parameter property (`private readonly http`, marked
; by an accessibility / `override` / `readonly` modifier — a bare parameter is not
; a field). They are what the `this.x` capture in references.scm binds to. A
; field initialised with an arrow function is a field, not a function.
;
; Both patterns are anchored to the body of a `class_declaration`. A field of an
; abstract class (an `abstract_class_declaration`, a Class since S-532 but one whose
; fields are not captured yet) or of a class expression would otherwise be a
; file-scope `Field` — no class to own it, so nothing could bind it, yet it would
; join every bare-name candidate set — and a modifier-bearing parameter of an
; interface method, a function type or a function (none of them a constructor)
; would become a bogus field.

(function_declaration
  name: (identifier) @symbol.function)

(class_declaration
  name: (type_identifier) @symbol.class)

(interface_declaration
  name: (type_identifier) @symbol.interface)

(enum_declaration
  name: (identifier) @symbol.enum)

(type_alias_declaration
  name: (type_identifier) @symbol.type_alias)

(method_definition
  name: (property_identifier) @symbol.method)

(variable_declarator
  name: (identifier) @symbol.function
  value: (arrow_function))

(variable_declarator
  name: (identifier) @symbol.function
  value: (function_expression))

(abstract_class_declaration
  name: (type_identifier) @symbol.class)

(abstract_method_signature
  name: (property_identifier) @symbol.method) @item.signature

(interface_declaration
  body: (interface_body
    (method_signature
      name: (property_identifier) @symbol.method) @item.signature))

; A wrapped component, at any depth like the arrow bindings above: `React.memo(…)`
; here and a bare `memo(…)` in the next pattern. `React` is matched by name, so
; `other.memo(…)` is no component.
(variable_declarator
  name: (identifier) @symbol.function
  value: (call_expression
    function: (member_expression
      object: (identifier) @_react
      property: (property_identifier) @_wrapper))
  (#match? @symbol.function "^[A-Z]")
  (#match? @_wrapper "^(forwardRef|memo)$")
  (#eq? @_react "React"))

(variable_declarator
  name: (identifier) @symbol.function
  value: (call_expression
    function: (identifier) @_wrapper)
  (#match? @symbol.function "^[A-Z]")
  (#match? @_wrapper "^(forwardRef|memo)$"))

(variable_declarator
  name: (identifier) @symbol.function
  value: (call_expression
    function: [(member_expression
                 object: (identifier) @_styled)
               (call_expression
                 function: (identifier) @_styled)]
    arguments: (template_string))
  (#match? @symbol.function "^[A-Z]")
  (#eq? @_styled "styled"))

; Top-level bindings: a `const` is a Constant, a `let`/`var` a Variable.
(program
  [(lexical_declaration
     "const"
     (variable_declarator
       name: (identifier) @symbol.constant))
   (export_statement
     declaration: (lexical_declaration
       "const"
       (variable_declarator
         name: (identifier) @symbol.constant)))])

(program
  [(lexical_declaration
     "let"
     (variable_declarator
       name: (identifier) @symbol.variable))
   (variable_declaration
     (variable_declarator
       name: (identifier) @symbol.variable))
   (export_statement
     declaration: [(lexical_declaration
                     "let"
                     (variable_declarator
                       name: (identifier) @symbol.variable))
                   (variable_declaration
                     (variable_declarator
                       name: (identifier) @symbol.variable))])])

; `export default <expression>`: the symbol is named by the `default` keyword.
(program
  (export_statement
    "default" @symbol.constant
    value: (_)))

(class_declaration
  body: (class_body
    (public_field_definition
      name: [(property_identifier) (private_property_identifier)] @symbol.field)))

(class_declaration
  body: (class_body
    (method_definition
      parameters: (formal_parameters
        [(required_parameter
           [(accessibility_modifier) (override_modifier) "readonly"]
           pattern: (identifier) @symbol.field)
         (optional_parameter
           [(accessibility_modifier) (override_modifier) "readonly"]
           pattern: (identifier) @symbol.field)]))))

; ── Parameter range (S-591, CR-190, FR-EX-32) ────────────────────────────────
; The `@arity.*` vocabulary the extraction engine reads (`extract::arity`): a
; callable's parameter list, and each parameter as required, optional (a
; default value raises only the maximum), variadic (an unbounded maximum), a
; receiver (not counted) or no parameter at all (skip). A list child no capture
; covers, or one captured `@arity.unknown`, records the range unknown rather
; than miscounting it. Captures never name a declaration, so every symbol and
; node is unchanged.
; A parameter with a default or a `?` is optional; `...xs` is variadic; a `this`
; parameter is the receiver. A lone arrow parameter (`x => …`) is its own list.
(function_declaration
  parameters: (formal_parameters) @arity.parameters)
(method_definition
  parameters: (formal_parameters) @arity.parameters)
(abstract_method_signature
  parameters: (formal_parameters) @arity.parameters)
(interface_declaration
  body: (interface_body
    (method_signature
      parameters: (formal_parameters) @arity.parameters)))
(variable_declarator
  value: (arrow_function
    parameters: (formal_parameters) @arity.parameters))
(variable_declarator
  value: (arrow_function
    parameter: (identifier) @arity.parameters @arity.required))
(variable_declarator
  value: (function_expression
    parameters: (formal_parameters) @arity.parameters))
(formal_parameters (required_parameter) @arity.required)
(formal_parameters (required_parameter value: (_)) @arity.optional)
(formal_parameters (optional_parameter) @arity.optional)
(formal_parameters (required_parameter pattern: (rest_pattern)) @arity.variadic)
(formal_parameters (required_parameter pattern: (this)) @arity.receiver)
