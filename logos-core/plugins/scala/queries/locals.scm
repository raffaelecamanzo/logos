; Scala local bindings (S-593, CR-191, FR-EX-33): every name a callable body
; binds, with its kind, scope and position. The vocabulary — sites, names,
; scope companions, `.hoisted` — is documented in `extract::locals`; captures
; here record facts only, never a node, edge or reference.
;
; A definition in a block — `val`, `var`, `def` — is in force throughout the
; block (`.hoisted`): a reference before it is a forward-reference error, never
; a read of an outer name. A local `import` binds from its position on. In a
; pattern, a name starting with an upper-case letter is a stable identifier (a
; constant, an object), never a binding, by the language's own rule.

; Callables: a method, a lambda.
(function_definition) @local.callable
(lambda_expression) @local.callable

; Parameters, scoped to their callable — every parameter list of a method,
; matched field-less: a field pattern sees only a field's first node.
(function_definition (parameters) @local.parameter) @local.parameter.scope
(lambda_expression parameters: (_) @local.parameter) @local.parameter.scope

(parameter name: (identifier) @local.name)
(binding name: (identifier) @local.name)
(lambda_expression parameters: (identifier) @local.name)

; `val` / `var` / `def` in a block — braced, or Scala 3's indented one: the
; block's, throughout it.
(block (val_definition pattern: (_) @local.variable) @local.variable.declaration @local.variable.hoisted)
(block (var_definition pattern: (_) @local.variable) @local.variable.declaration @local.variable.hoisted)
(block (function_definition name: (identifier) @local.variable @local.name) @local.variable.declaration @local.variable.hoisted)
(indented_block (val_definition pattern: (_) @local.variable) @local.variable.declaration @local.variable.hoisted)
(indented_block (var_definition pattern: (_) @local.variable) @local.variable.declaration @local.variable.hoisted)
(indented_block (function_definition name: (identifier) @local.variable @local.name) @local.variable.declaration @local.variable.hoisted)

; A `for` enumerator, a `case` clause: the comprehension's, the clause's.
(for_expression enumerators: (enumerators) @local.variable) @local.variable.scope
(case_clause pattern: (_) @local.variable) @local.variable.scope

; The names a pattern binds.
((_ pattern: (identifier) @local.name)
  (#match? @local.name "^[a-z]"))
((tuple_pattern (identifier) @local.name)
  (#match? @local.name "^[a-z]"))
((infix_pattern left: (identifier) @local.name)
  (#match? @local.name "^[a-z]"))
((infix_pattern right: (identifier) @local.name)
  (#match? @local.name "^[a-z]"))
(capture_pattern name: (identifier) @local.name)
((enumerator . (identifier) @local.name)
  (#match? @local.name "^[a-z]"))

; A local `import`: its block's, from its position on. `import a.b.c` binds
; `c`; a selector list binds each name or its alias; a wildcard binds none.
(block (import_declaration) @local.import @local.import.declaration)
(indented_block (import_declaration) @local.import @local.import.declaration)

(import_declaration (identifier) @local.name .)
(namespace_selectors (identifier) @local.name)
(arrow_renamed_identifier alias: (identifier) @local.name)
