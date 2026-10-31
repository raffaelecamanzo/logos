; Go local bindings (S-593, CR-191, FR-EX-33): every name a callable body
; binds, with its kind, scope and position. The vocabulary — sites, names,
; scope companions, `.hoisted` — is documented in `extract::locals`; captures
; here record facts only, never a node, edge or reference.
;
; Go binds from its position on: no binding is hoisted. The blank identifier
; `_` binds nothing. A method's receiver (`func (s *Svc) M()`) is NOT a local
; binding: it is the name `@ref.receiver.self_name` reads, so a call on it stays
; the method's own.

; Callables: a function, a method, a function literal.
(function_declaration) @local.callable
(method_declaration) @local.callable
(func_literal) @local.callable

; Parameters and named results, scoped to their callable.
(function_declaration parameters: (parameter_list) @local.parameter) @local.parameter.scope
(function_declaration result: (parameter_list) @local.parameter) @local.parameter.scope
(method_declaration parameters: (parameter_list) @local.parameter) @local.parameter.scope
(method_declaration result: (parameter_list) @local.parameter) @local.parameter.scope
(func_literal parameters: (parameter_list) @local.parameter) @local.parameter.scope
(func_literal result: (parameter_list) @local.parameter) @local.parameter.scope

; `a, b int` names both in one declaration, and a field pattern sees only a
; field's first node, so names are matched field-less: a Go type is never a
; plain `identifier`.
((parameter_declaration (identifier) @local.name)
  (#not-eq? @local.name "_"))
((variadic_parameter_declaration name: (identifier) @local.name)
  (#not-eq? @local.name "_"))

; A `for` clause's `i := 0` is scoped to its loop, a `range` clause's `k, v :=`
; to its loop, an `if` / `switch` initializer's to its statement, a type
; switch's `v := x.(type)` to the switch, a `select` case's `v := <-ch` to its
; case. Written before the general `:=` below, which they would otherwise match.
(for_statement
  (for_clause
    initializer: (short_var_declaration left: (expression_list) @local.variable))) @local.variable.scope
(for_statement
  (range_clause left: (expression_list) @local.variable ":=")) @local.variable.scope
(type_switch_statement alias: (expression_list) @local.variable) @local.variable.scope
(communication_case
  communication: (receive_statement left: (expression_list) @local.variable ":=")) @local.variable.scope

; `a, b := …` in a block, and an `if` / `switch` initializer's: scoped to the
; node holding the statement — the block's statement list, or the conditional.
(short_var_declaration left: (expression_list) @local.variable) @local.variable.declaration

; The names of a binding list: its left-hand side only, never the identifiers
; an initializer or a function literal's body reads.
((short_var_declaration left: (expression_list (identifier) @local.name))
  (#not-eq? @local.name "_"))
((range_clause left: (expression_list (identifier) @local.name))
  (#not-eq? @local.name "_"))
((type_switch_statement alias: (expression_list (identifier) @local.name))
  (#not-eq? @local.name "_"))
((receive_statement left: (expression_list (identifier) @local.name))
  (#not-eq? @local.name "_"))

; `var x T` / `const k = 1` inside a body (a top-level one is in no callable).
; The names are a spec's own identifiers — its initializer is an
; `expression_list`, whose identifiers are reads.
(var_declaration) @local.variable @local.variable.declaration
(const_declaration) @local.variable @local.variable.declaration

((var_spec (identifier) @local.name)
  (#not-eq? @local.name "_"))
((const_spec (identifier) @local.name)
  (#not-eq? @local.name "_"))
