; C++ local bindings (S-593, CR-191, FR-EX-33): every name a callable body
; binds, with its kind, scope and position. The vocabulary — sites, names,
; scope companions, `.hoisted` — is documented in `extract::locals`; captures
; here record facts only, never a node, edge or reference.
;
; C++ binds from the point of declaration on: no binding is hoisted. A
; block-scope `using ns::f;` imports `f` into its block; `using namespace ns;`
; names no single binding and records none. A lambda's capture list re-uses
; outer names and binds none of its own.

; Callables: a function definition, a lambda.
(function_definition) @local.callable
(lambda_expression) @local.callable

; Parameters, scoped to their callable — through a pointer or reference return
; declarator too.
(function_definition
  declarator: (function_declarator parameters: (parameter_list) @local.parameter)) @local.parameter.scope
(function_definition
  declarator: (_ (function_declarator parameters: (parameter_list) @local.parameter))) @local.parameter.scope
; `char **f(…)`, `T ***f(…)`: a pointer chain, followed by its `declarator:`
; field only, so a function-pointer parameter's own list is never reached.
(function_definition
  declarator: (pointer_declarator
    declarator: (pointer_declarator
      declarator: (function_declarator parameters: (parameter_list) @local.parameter)))) @local.parameter.scope
(function_definition
  declarator: (pointer_declarator
    declarator: (pointer_declarator
      declarator: (pointer_declarator
        declarator: (function_declarator parameters: (parameter_list) @local.parameter))))) @local.parameter.scope
(lambda_expression
  declarator: (abstract_function_declarator parameters: (parameter_list) @local.parameter)) @local.parameter.scope

; An `if` / `switch` initializer, an `if` / `switch` / `while` condition
; declaration, a `for` initializer, a range-`for` variable, a `catch`
; parameter: the statement's. Written before the general declaration below,
; which would scope a condition declaration to its parentheses.
(if_statement
  condition: (condition_clause initializer: (init_statement (declaration) @local.variable))) @local.variable.scope
(if_statement
  condition: (condition_clause value: (declaration) @local.variable)) @local.variable.scope
(switch_statement
  condition: (condition_clause initializer: (init_statement (declaration) @local.variable))) @local.variable.scope
(switch_statement
  condition: (condition_clause value: (declaration) @local.variable)) @local.variable.scope
(while_statement
  condition: (condition_clause value: (declaration) @local.variable)) @local.variable.scope
(for_statement initializer: (declaration) @local.variable) @local.variable.scope
(for_range_loop declarator: (_) @local.variable) @local.variable.scope
(catch_clause parameters: (parameter_list) @local.variable) @local.variable.scope

; A declaration (structured bindings included): its block's.
(declaration) @local.variable @local.variable.declaration

; A block-scope `using ns::f;`: its block's.
(compound_statement (using_declaration) @local.import @local.import.declaration)

; The names a declarator binds: the identifier at the end of its chain.
(_ declarator: (identifier) @local.name)
(reference_declarator (identifier) @local.name)
(structured_binding_declarator (identifier) @local.name)

(using_declaration (qualified_identifier name: (identifier) @local.name))
(using_declaration (qualified_identifier name: (qualified_identifier name: (identifier) @local.name)))
(using_declaration
  (qualified_identifier name: (qualified_identifier name: (qualified_identifier name: (identifier) @local.name))))
