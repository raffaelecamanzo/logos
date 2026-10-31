; C local bindings (S-593, CR-191, FR-EX-33): every name a function body
; binds, with its kind, scope and position. The vocabulary — sites, names,
; scope companions, `.hoisted` — is documented in `extract::locals`; captures
; here record facts only, never a node, edge or reference.
;
; C binds from the point of declaration on: no binding is hoisted.

; Callables: a function definition.
(function_definition) @local.callable

; Parameters, scoped to their function — through a pointer return declarator
; too.
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

; A `for` initializer: the loop's. Written before the general declaration below.
(for_statement initializer: (declaration) @local.variable) @local.variable.scope

; A declaration: its block's.
(declaration) @local.variable @local.variable.declaration

; The names a declarator binds: the identifier at the end of its chain.
(_ declarator: (identifier) @local.name)
