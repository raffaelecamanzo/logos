; Kotlin local bindings (S-593, CR-191, FR-EX-33): every name a callable body
; binds, with its kind, scope and position. The vocabulary — sites, names,
; scope companions, `.hoisted` — is documented in `extract::locals`; captures
; here record facts only, never a node, edge or reference.
;
; Kotlin binds from its position on: no binding is hoisted, a local `fun`
; included. A lambda's implicit `it` is written nowhere and is not recorded.
; Kotlin has no local import.

; Callables: a function, an anonymous function, a lambda, a constructor, an
; accessor, an `init` block.
(function_declaration) @local.callable
(anonymous_function) @local.callable
(lambda_literal) @local.callable
(secondary_constructor) @local.callable
(getter) @local.callable
(setter) @local.callable
(anonymous_initializer) @local.callable

; Parameters, scoped to their callable.
(function_declaration (function_value_parameters) @local.parameter) @local.parameter.scope
(anonymous_function (function_value_parameters) @local.parameter) @local.parameter.scope
(secondary_constructor (function_value_parameters) @local.parameter) @local.parameter.scope
(setter (identifier) @local.parameter @local.name) @local.parameter.scope
(lambda_literal (lambda_parameters) @local.parameter) @local.parameter.scope

(parameter . (identifier) @local.name)
(variable_declaration . (identifier) @local.name)

; `val` / `var` in a block or a lambda body: its block's.
(block (property_declaration) @local.variable @local.variable.declaration)
(lambda_literal (property_declaration) @local.variable @local.variable.declaration)

; A `for`, a `catch`, a `when (val s = …)` subject: the statement's.
(for_statement . (variable_declaration) @local.variable) @local.variable.scope
(for_statement . (multi_variable_declaration) @local.variable) @local.variable.scope
(catch_block . (identifier) @local.variable @local.name) @local.variable.scope
(when_expression (when_subject (variable_declaration) @local.variable)) @local.variable.scope

; A local `fun`: its block's, from its declaration on.
(block (function_declaration name: (identifier) @local.variable @local.name) @local.variable.declaration)
(lambda_literal (function_declaration name: (identifier) @local.variable @local.name) @local.variable.declaration)
