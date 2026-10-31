; TypeScript / JavaScript local bindings (S-593, CR-191, FR-EX-33): every name a
; callable body binds, with its kind, scope and position. The vocabulary —
; sites, names, scope companions, `.hoisted` — is documented in
; `extract::locals`; captures here record facts only, never a node, edge or
; reference. `typescript/queries/locals.scm` is this file's twin: keep their patterns identical.
;
; Hoisting is the language's: a `var` binds for its whole function, and a
; function declaration for its whole block. A `let` / `const` / `class` binds
; for its whole block too: before its declaration the name is in its temporal
; dead zone, so a read throws rather than reaching an outer name — and a
; function declared earlier in the block reads the local when it runs.
; Parameters, loop, `catch` and destructured names bind from their position.

; Callables: every function form with a body.
(function_declaration) @local.callable
(function_expression) @local.callable
(generator_function_declaration) @local.callable
(generator_function) @local.callable
(arrow_function) @local.callable
(method_definition) @local.callable

; Parameters, scoped to their callable.
(function_declaration parameters: (formal_parameters) @local.parameter) @local.parameter.scope
(function_expression parameters: (formal_parameters) @local.parameter) @local.parameter.scope
(generator_function_declaration parameters: (formal_parameters) @local.parameter) @local.parameter.scope
(generator_function parameters: (formal_parameters) @local.parameter) @local.parameter.scope
(arrow_function parameters: (formal_parameters) @local.parameter) @local.parameter.scope
(arrow_function parameter: (identifier) @local.parameter @local.name) @local.parameter.scope
(method_definition parameters: (formal_parameters) @local.parameter) @local.parameter.scope

; `var`: the whole function's.
(variable_declaration
  (variable_declarator name: (_) @local.variable)) @local.variable.hoisted
; `let` / `const`: the block's — or the loop's, for a `for` initializer.
(lexical_declaration
  (variable_declarator name: (_) @local.variable)) @local.variable.declaration @local.variable.hoisted

; `for (const x of xs)` / `for (let k in o)`: the loop's; `for (var k in o)`:
; the function's. A `for (x of xs)` with no keyword assigns, binding nothing.
(for_in_statement kind: ["let" "const"] left: (_) @local.variable) @local.variable.scope
(for_in_statement kind: "var" left: (_) @local.variable @local.variable.hoisted)

; `catch (e)`: the clause's.
(catch_clause parameter: (_) @local.variable) @local.variable.scope

; A function or class declared in a block: the block's.
(statement_block
  (function_declaration name: (identifier) @local.variable @local.name) @local.variable.declaration @local.variable.hoisted)
(statement_block
  (generator_function_declaration name: (identifier) @local.variable @local.name) @local.variable.declaration @local.variable.hoisted)
(statement_block
  (class_declaration name: (type_identifier) @local.variable @local.name) @local.variable.declaration @local.variable.hoisted)

; The names a parameter, declarator or pattern binds.
(required_parameter pattern: (identifier) @local.name)
(optional_parameter pattern: (identifier) @local.name)
(variable_declarator name: (identifier) @local.name)
(for_in_statement left: (identifier) @local.name)
(catch_clause parameter: (identifier) @local.name)
(object_pattern (shorthand_property_identifier_pattern) @local.name)
(object_assignment_pattern left: (shorthand_property_identifier_pattern) @local.name)
(pair_pattern value: (identifier) @local.name)
(array_pattern (identifier) @local.name)
(assignment_pattern left: (identifier) @local.name)
(rest_pattern (identifier) @local.name)
