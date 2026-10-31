; Ruby local bindings (S-593, CR-191, FR-EX-33): every name a callable body
; binds, with its kind, scope and position. The vocabulary — sites, names,
; scope companions, `.hoisted` — is documented in `extract::locals`; captures
; here record facts only, never a node, edge or reference.
;
; A Ruby local is its method's, block's or lambda's (no other construct opens a
; scope), so every binding but a parameter takes the innermost callable as its
; scope (no companion). It binds from its position on: the parser reads a name
; as a local only after an assignment to it. Instance, class and global
; variables are not locals. Ruby has no import: `require` is a call.

; Callables: a method, a block, a lambda.
(method) @local.callable
(singleton_method) @local.callable
(block) @local.callable
(do_block) @local.callable
(lambda) @local.callable

; Parameters, scoped to their callable.
(method parameters: (method_parameters) @local.parameter) @local.parameter.scope
(singleton_method parameters: (method_parameters) @local.parameter) @local.parameter.scope
(block parameters: (block_parameters) @local.parameter) @local.parameter.scope
(do_block parameters: (block_parameters) @local.parameter) @local.parameter.scope
(lambda parameters: (lambda_parameters) @local.parameter) @local.parameter.scope

(method_parameters (identifier) @local.name)
(block_parameters (identifier) @local.name)
(lambda_parameters (identifier) @local.name)
(optional_parameter name: (identifier) @local.name)
(keyword_parameter name: (identifier) @local.name)
(splat_parameter name: (identifier) @local.name)
(hash_splat_parameter name: (identifier) @local.name)
(block_parameter name: (identifier) @local.name)
(destructured_parameter (identifier) @local.name)

; Assignment targets, `for` variables, `rescue => e`, `in` patterns: the
; callable's.
(assignment left: (_) @local.variable)
(operator_assignment left: (identifier) @local.variable @local.name)
(for pattern: (_) @local.variable)
(exception_variable (identifier) @local.variable @local.name)
(in_clause pattern: (_) @local.variable)

(assignment left: (identifier) @local.name)
(left_assignment_list (identifier) @local.name)
(destructured_left_assignment (identifier) @local.name)
(rest_assignment (identifier) @local.name)
(for pattern: (identifier) @local.name)
(in_clause pattern: (identifier) @local.name)
(array_pattern (identifier) @local.name)
(find_pattern (identifier) @local.name)
(keyword_pattern value: (identifier) @local.name)
(as_pattern name: (identifier) @local.name)
