; Rust local bindings (S-593, CR-191, FR-EX-33): every name a callable body
; binds, with its kind, scope and position. The vocabulary — sites, names,
; scope companions, `.hoisted` — is documented in `extract::locals`; captures
; here record facts only, never a node, edge or reference. The receiver typing
; markers of `references.scm` (`@ref.receiver.binding`, S-587) state the same
; scopes for their own purpose and are untouched by this file.
;
; A `let` binds from its position on; an item inside a block — a `use`, a
; `fn` — is in force throughout the block (`.hoisted`). `self` is no local
; binding. In a pattern, an identifier starting with an upper-case letter is a
; unit variant or a constant (`None`, `MAX`), never a binding, by the
; language's naming convention: only lower-case and `_`-led names are captured.

; Callables: a function, a closure.
(function_item) @local.callable
(closure_expression) @local.callable

; Parameters, scoped to their callable.
(function_item parameters: (parameters) @local.parameter) @local.parameter.scope
(closure_expression parameters: (closure_parameters) @local.parameter) @local.parameter.scope

; `let` binds in its block, from its position on — a `let … else` too.
(let_declaration pattern: (_) @local.variable) @local.variable.declaration

; A loop, a match arm, an `if let` / `while let` condition: scoped to the
; expression or arm.
(for_expression pattern: (_) @local.variable) @local.variable.scope
(match_arm pattern: (_) @local.variable) @local.variable.scope
(if_expression condition: (_) @local.variable) @local.variable.scope
(while_expression condition: (_) @local.variable) @local.variable.scope

; Items in a block: a `use` imports for the whole block, a nested `fn` names
; itself for the whole block.
(block
  (use_declaration argument: (_) @local.import) @local.import.declaration @local.import.hoisted)
(block
  (function_item name: (identifier) @local.variable @local.name) @local.variable.declaration @local.variable.hoisted)

; The names a pattern binds.
((_ pattern: (identifier) @local.name)
  (#match? @local.name "^[a-z_]"))
((closure_parameters (identifier) @local.name)
  (#match? @local.name "^[a-z_]"))
((tuple_pattern (identifier) @local.name)
  (#match? @local.name "^[a-z_]"))
((slice_pattern (identifier) @local.name)
  (#match? @local.name "^[a-z_]"))
((tuple_struct_pattern "(" (identifier) @local.name)
  (#match? @local.name "^[a-z_]"))
((or_pattern (identifier) @local.name)
  (#match? @local.name "^[a-z_]"))
((match_pattern . (identifier) @local.name)
  (#match? @local.name "^[a-z_]"))
((captured_pattern . (identifier) @local.name)
  (#match? @local.name "^[a-z_]"))
((mut_pattern (identifier) @local.name)
  (#match? @local.name "^[a-z_]"))
((ref_pattern (identifier) @local.name)
  (#match? @local.name "^[a-z_]"))
((reference_pattern (identifier) @local.name)
  (#match? @local.name "^[a-z_]"))
(field_pattern name: (shorthand_field_identifier) @local.name)

; The names a `use` brings in: its last segment, an alias, each list item.
; `use m::*` names none.
(use_declaration argument: (identifier) @local.name)
(use_declaration argument: (scoped_identifier name: (identifier) @local.name))
(use_as_clause alias: (identifier) @local.name)
(use_list (identifier) @local.name)
(use_list (scoped_identifier name: (identifier) @local.name))
