; Python local bindings (S-593, CR-191, FR-EX-33): every name a callable body
; binds, with its kind, scope and position. The vocabulary — sites, names,
; scope companions, `.hoisted` — is documented in `extract::locals`; captures
; here record facts only, never a node, edge or reference.
;
; A Python local is the FUNCTION's: a name the body assigns anywhere — an
; assignment, a loop, `with … as`, `except … as`, a walrus, a `match` capture,
; an import, a nested `def` or `class` — is local throughout the function, and
; a read before the assignment raises rather than reaching the module's name.
; So every such binding takes the callable as its scope (no companion) and is
; `.hoisted`. Only a comprehension's own variable is scoped to the
; comprehension, and a parameter is in force from the start anyway.
;
; `global` / `nonlocal` names are not local bindings and are not captured. An
; assignment to such a name is still recorded as a local (CR-191 §3.3 leaves
; their semantics out of scope), and so is an assignment in the body of a class
; a function declares. A method's `self` is a parameter like any other: the
; language spells its receiver as one.

; Callables: a function, a lambda.
(function_definition) @local.callable
(lambda) @local.callable

; Parameters, scoped to their callable.
(function_definition parameters: (parameters) @local.parameter) @local.parameter.scope
(lambda parameters: (lambda_parameters) @local.parameter) @local.parameter.scope

(parameters (identifier) @local.name)
(lambda_parameters (identifier) @local.name)
(typed_parameter . (identifier) @local.name)
(default_parameter name: (identifier) @local.name)
(typed_default_parameter name: (identifier) @local.name)
(list_splat_pattern (identifier) @local.name)
(dictionary_splat_pattern (identifier) @local.name)

; A comprehension's variables: scoped to the comprehension.
(_ (for_in_clause left: (_) @local.variable)) @local.variable.scope
(for_in_clause left: (identifier) @local.name)

; Assignment targets, loop variables, `with` / `except` targets, walrus names:
; the function's, throughout it.
(assignment left: (_) @local.variable @local.variable.hoisted)
(augmented_assignment left: (identifier) @local.variable @local.variable.hoisted)
(for_statement left: (_) @local.variable @local.variable.hoisted)
(as_pattern alias: (as_pattern_target) @local.variable @local.variable.hoisted)
(named_expression name: (identifier) @local.variable @local.variable.hoisted)

(assignment left: (identifier) @local.name)
(augmented_assignment left: (identifier) @local.name)
(for_statement left: (identifier) @local.name)
(named_expression name: (identifier) @local.name)
(pattern_list (identifier) @local.name)
(tuple_pattern (identifier) @local.name)
(list_pattern (identifier) @local.name)
(as_pattern_target (identifier) @local.name)
(as_pattern_target (tuple (identifier) @local.name))
(as_pattern_target (list (identifier) @local.name))

; `match` captures: a bare name (a one-identifier `dotted_name`), a `*rest` /
; `**rest`, an `as` alias. `_` is the wildcard and binds nothing.
(case_clause (case_pattern) @local.variable @local.variable.hoisted)

((case_pattern (dotted_name . (identifier) @local.name .))
  (#not-eq? @local.name "_"))
((keyword_pattern (dotted_name . (identifier) @local.name .))
  (#not-eq? @local.name "_"))
((splat_pattern (identifier) @local.name)
  (#not-eq? @local.name "_"))
(as_pattern (case_pattern) (identifier) @local.name)

; Imports inside a function: `import a.b` binds `a`, `import a.b as c` binds
; `c`, `from m import f` binds `f`, `from m import f as g` binds `g`.
(import_statement) @local.import @local.import.hoisted
(import_from_statement) @local.import @local.import.hoisted

; A field pattern sees only a field's first node, so the imported names are
; matched field-less: a `from` statement's after its `import` keyword, past the
; module it names.
(import_statement (dotted_name . (identifier) @local.name))
(import_from_statement "import" (dotted_name (identifier) @local.name))
(aliased_import alias: (identifier) @local.name)

; A `def` or `class` a function body declares names itself in that function:
; its scope is the outer function (`.scope`), never the `def` itself.
(function_definition
  body: (block
    (function_definition name: (identifier) @local.variable @local.name) @local.variable.hoisted)) @local.variable.scope
(function_definition
  body: (block
    (decorated_definition
      definition: (function_definition name: (identifier) @local.variable @local.name)) @local.variable.hoisted)) @local.variable.scope
(function_definition
  body: (block
    (class_definition name: (identifier) @local.variable @local.name) @local.variable.hoisted)) @local.variable.scope
