; Java local bindings (S-593, CR-191, FR-EX-33): every name a callable body
; binds, with its kind, scope and position. The vocabulary — sites, names,
; scope companions, `.hoisted` — is documented in `extract::locals`; captures
; here record facts only, never a node, edge or reference.
;
; Java binds from its position on: no binding is hoisted. Java has no local
; import.

; Callables: a method, a constructor, a lambda, an initializer.
(method_declaration) @local.callable
(constructor_declaration) @local.callable
(compact_constructor_declaration) @local.callable
(lambda_expression) @local.callable
(static_initializer) @local.callable

(block) @local.block
(switch_block_statement_group) @local.block
(switch_rule) @local.block

; Parameters, scoped to their callable.
(method_declaration parameters: (formal_parameters) @local.parameter) @local.parameter.scope
(constructor_declaration parameters: (formal_parameters) @local.parameter) @local.parameter.scope
(lambda_expression parameters: (_) @local.parameter) @local.parameter.scope

(formal_parameter name: (identifier) @local.name)
(spread_parameter (variable_declarator name: (identifier) @local.name))
(inferred_parameters (identifier) @local.name)
(lambda_expression parameters: (identifier) @local.name)

; A local declaration: its block's — or its loop's, for a `for` initializer.
(local_variable_declaration) @local.variable @local.variable.declaration
(local_variable_declaration declarator: (variable_declarator name: (identifier) @local.name))

; An enhanced `for`, a `catch`, a try-with-resources: the statement's.
(enhanced_for_statement name: (identifier) @local.variable @local.name) @local.variable.scope
(catch_clause (catch_formal_parameter name: (identifier) @local.variable @local.name)) @local.variable.scope
(try_with_resources_statement
  resources: (resource_specification (resource name: (identifier) @local.variable @local.name))) @local.variable.scope

; A pattern's names: an `instanceof` pattern — a type pattern's name or a
; record pattern's components — binds in the block holding it (the language
; scopes it by flow, never beyond its block); a `switch` pattern binds in its
; case.
(instanceof_expression name: (identifier) @local.variable @local.name @local.variable.block)
(instanceof_expression pattern: (record_pattern) @local.variable @local.variable.block)
(switch_label (pattern) @local.variable @local.variable.block)

(type_pattern (identifier) @local.name)
(record_pattern_component (identifier) @local.name)
