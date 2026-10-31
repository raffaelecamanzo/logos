; PHP local bindings (S-593, CR-191, FR-EX-33): every name a callable body
; binds, with its kind, scope and position. The vocabulary — sites, names,
; scope companions, `.hoisted` — is documented in `extract::locals`; captures
; here record facts only, never a node, edge or reference.
;
; A PHP variable is its function's: no block scopes it, so every binding takes
; the callable as its scope (no companion) and binds from its position on. A
; name keeps its `$`. A closure's `use ($v)` captures are its parameters. A
; `global $g` names no local binding; `$this` is never assigned.

; Callables: a function, a method, a closure, an arrow function.
(function_definition) @local.callable
(method_declaration) @local.callable
(anonymous_function) @local.callable
(arrow_function) @local.callable

; Parameters, scoped to their callable — and a closure's `use` captures.
(function_definition parameters: (formal_parameters) @local.parameter) @local.parameter.scope
(method_declaration parameters: (formal_parameters) @local.parameter) @local.parameter.scope
(anonymous_function parameters: (formal_parameters) @local.parameter) @local.parameter.scope
(arrow_function parameters: (formal_parameters) @local.parameter) @local.parameter.scope
(anonymous_function (anonymous_function_use_clause) @local.parameter) @local.parameter.scope

(simple_parameter name: (variable_name) @local.name)
(variadic_parameter name: (variable_name) @local.name)
(property_promotion_parameter name: (variable_name) @local.name)
(anonymous_function_use_clause (variable_name) @local.name)
(anonymous_function_use_clause (by_ref (variable_name) @local.name))

; Assignment targets, `foreach` variables, `catch` variables, `static`
; variables: the function's.
(assignment_expression left: (_) @local.variable)
(reference_assignment_expression left: (_) @local.variable)
(foreach_statement) @local.variable
(catch_clause name: (variable_name) @local.variable @local.name)
(static_variable_declaration name: (variable_name) @local.variable @local.name)

(assignment_expression left: (variable_name) @local.name)
(reference_assignment_expression left: (variable_name) @local.name)
(list_literal (variable_name) @local.name)
(list_literal (by_ref (variable_name) @local.name))
(foreach_statement . (_) . (variable_name) @local.name)
(foreach_statement . (_) . (by_ref (variable_name) @local.name))
(foreach_statement . (_) . (pair (variable_name) @local.name))
(foreach_statement . (_) . (pair (by_ref (variable_name) @local.name)))
