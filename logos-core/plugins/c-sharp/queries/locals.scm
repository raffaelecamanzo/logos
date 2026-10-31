; C# local bindings (S-593, CR-191, FR-EX-33): every name a callable body
; binds, with its kind, scope and position. The vocabulary — sites, names,
; scope companions, `.hoisted` — is documented in `extract::locals`; captures
; here record facts only, never a node, edge or reference.
;
; A C# local's scope is its whole block (`.hoisted`): using it before its
; declaration is an error, never a read of an outer name — and a local function
; may be called anywhere in its block. A pattern or `out var` variable leaks
; into the block holding its statement, or its `switch` section. `using`
; directives cannot appear in a body; a `using` statement's or declaration's
; variable is a local like any other.

; Callables: a method, a constructor, a finalizer, an operator, an accessor, a
; local function, a lambda, an anonymous method.
(method_declaration) @local.callable
(constructor_declaration) @local.callable
(destructor_declaration) @local.callable
(operator_declaration) @local.callable
(conversion_operator_declaration) @local.callable
(accessor_declaration) @local.callable
(local_function_statement) @local.callable
(lambda_expression) @local.callable
(anonymous_method_expression) @local.callable

(block) @local.block
(switch_section) @local.block
(switch_expression_arm) @local.block

; Parameters, scoped to their callable.
(method_declaration parameters: (parameter_list) @local.parameter) @local.parameter.scope
(constructor_declaration parameters: (parameter_list) @local.parameter) @local.parameter.scope
(operator_declaration parameters: (parameter_list) @local.parameter) @local.parameter.scope
(conversion_operator_declaration parameters: (parameter_list) @local.parameter) @local.parameter.scope
(local_function_statement parameters: (parameter_list) @local.parameter) @local.parameter.scope
(lambda_expression parameters: (_) @local.parameter) @local.parameter.scope
(anonymous_method_expression parameters: (parameter_list) @local.parameter) @local.parameter.scope

(parameter name: (identifier) @local.name)
(parameter_list name: (identifier) @local.name)
(implicit_parameter) @local.name

; A `for` initializer, a `using` statement, a `foreach`, a `catch`: the
; statement's. Written before the general declarations below.
(for_statement initializer: (variable_declaration) @local.variable @local.variable.hoisted) @local.variable.scope
(using_statement (variable_declaration) @local.variable @local.variable.hoisted) @local.variable.scope
(foreach_statement left: (_) @local.variable @local.variable.hoisted) @local.variable.scope
(catch_clause (catch_declaration) @local.variable @local.variable.hoisted) @local.variable.scope

; A local declaration (a `using var` one too): its block's, throughout it.
(local_declaration_statement) @local.variable @local.variable.declaration @local.variable.hoisted

; A local function: its block's, throughout it.
(local_function_statement name: (identifier) @local.variable @local.name) @local.variable.declaration @local.variable.hoisted

; A deconstruction, an `out var`, a pattern variable: the block or `switch`
; section holding it, throughout it.
(declaration_expression) @local.variable @local.variable.block @local.variable.hoisted
(declaration_pattern) @local.variable @local.variable.block @local.variable.hoisted
(var_pattern) @local.variable @local.variable.block @local.variable.hoisted

; A query's range variable: the query's.
(query_expression (from_clause name: (identifier) @local.variable @local.name)) @local.variable.scope

(variable_declarator name: (identifier) @local.name)
(tuple_pattern name: (identifier) @local.name)
(foreach_statement left: (identifier) @local.name)
(catch_declaration name: (identifier) @local.name)
(declaration_expression name: (identifier) @local.name)
(declaration_pattern name: (identifier) @local.name)
(var_pattern name: (identifier) @local.name)
(parenthesized_variable_designation (identifier) @local.name)
