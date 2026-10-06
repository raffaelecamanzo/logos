; Scala symbol-extraction query (S-061, CR-009, capability = "symbols").
;
; Capture names map to NodeKind via NodeKind::as_str (extract engine): the
; captured node is the declaration's *name* identifier, whose parent is the
; declaration the extractor records. Compiled against the built Scala Language
; at load; fails fast naming this file on drift (FR-PL-02). Droppable on disk at
; `.logos/plugins/scala/queries/symbols.scm` (FR-PL-04, UAT-PL-03).
;
; Capturing classes as NodeKind::Class is, per ADR-21, Scala's *class-bearing
; applicability declaration* — the kind a construct extracts to is what makes
; the Cohesion (LCOM4) and Focus structural metrics apply (FR-QM-11).
;
; A Scala `object` (singleton) is captured as a Class: it is a nominal,
; member-bearing construct referenced by name. A `class Foo` + companion
; `object Foo` therefore share a name; references to the bare `Foo` stay
; honestly ambiguous and unresolved under the binder's exactly-one rule
; (NFR-RA-05) — a measured limitation, never a fabricated edge.
;
; Member `val`/`var` are scoped to a `template_body` so local values inside
; method blocks are not mis-extracted as fields. Destructuring/tuple patterns
; (`val (a, b) = …`) and operator-named members are deliberately skipped in v1
; — best-effort, never fabricated.

(class_definition
  name: (identifier) @symbol.class)

(object_definition
  name: (identifier) @symbol.class)

(trait_definition
  name: (identifier) @symbol.trait)

(enum_definition
  name: (identifier) @symbol.enum)

(type_definition
  name: (type_identifier) @symbol.type_alias)

; `def` — a method (member) or a top-level/standalone function. Both extract to
; the callable scope the test-marker, complexity, nesting, and shape passes
; reason about. `function_declaration` is an abstract `def` in a trait.
(function_definition
  name: (identifier) @symbol.method)

(function_declaration
  name: (identifier) @symbol.method)

; Member values — direct children of a class/object/trait body only.
(template_body
  (val_definition
    pattern: (identifier) @symbol.field))

(template_body
  (var_definition
    pattern: (identifier) @symbol.field))

; The file's package (S-518, CR-170, FR-RS-13). Not a declaration — its capture
; group is `module`, not `symbol`, so the declaration walk skips it. Chained
; clauses (`package a` then `package b`) compose to `a.b`, as Scala reads them —
; the `@module.namespace.chained` marker says so; without it a second clause
; would replace the first, as a PHP `namespace` statement does. A
; `package a { … }` block scopes its body. Each top-level type is named that
; package plus its own name, whatever directory the file sits in.
(package_clause
  name: (package_identifier) @module.namespace @module.namespace.chained)

; ── Parameter range (S-591, CR-190, FR-EX-32) ────────────────────────────────
; The `@arity.*` vocabulary the extraction engine reads (`extract::arity`): a
; callable's parameter list, and each parameter as required, optional (a
; default value raises only the maximum), variadic (an unbounded maximum), a
; receiver (not counted) or no parameter at all (skip). A list child no capture
; covers, or one captured `@arity.unknown`, records the range unknown rather
; than miscounting it. Captures never name a declaration, so every symbol and
; node is unchanged.
; A declaration with several lists (`def f(a: Int)(b: Int)`) is admitted by its
; first, the one a call's first argument list fills: every list is captured
; (a `parameters:` field pattern would match the first alone) and the engine
; takes the first. `T*` is variadic; a `using` or `implicit` list is filled by
; the compiler, so its range is unknown.
(function_definition
  (parameters) @arity.parameters)
(function_declaration
  (parameters) @arity.parameters)
(parameters (parameter) @arity.required)
(parameters (parameter default_value: (_)) @arity.optional)
(parameters (parameter type: (repeated_parameter_type)) @arity.variadic)
(parameters "using" @arity.unknown)
(parameters "implicit" @arity.unknown)
