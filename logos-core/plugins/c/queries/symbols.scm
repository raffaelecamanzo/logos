; C symbol-extraction query (S-056, capability = "symbols").
;
; Capture names map to NodeKind via NodeKind::as_str (extract engine). Compiled
; against the built C Language on its first use, or at load as an
; on-disk override; drift fails naming this file (FR-PL-02, CR-197).
; Droppable on disk at `.logos/plugins/c/queries/symbols.scm` (FR-PL-04,
; UAT-PL-03).
;
; Honesty posture (NFR-CC-04): C is NOT class-applicable — no pattern emits a
; Class, so Cohesion (Class) stays honestly `n/a` for C (FR-QM-11, FR-CV-08 —
; n/a paths). A `struct`/`union` WITH a body is a Struct and its members are
; Fields (S-582, FR-EX-31): the declaration a C repository's headers make,
; which Focus (Class ∪ Struct) counts by its span, as it counts a Go type with
; no methods. A `struct X *p` reference or a `struct X;` forward declaration has
; no body and creates no node, and no pattern here emits a Method.

; Function *definitions* only — a bare prototype is not the symbol, its
; definition is. The name's `function_declarator` is nested under the
; `declarator` field of the definition, possibly through one or two
; `pointer_declarator`s for pointer-returning functions (`int *f()`,
; `char **g()`). All three return shapes are anchored to the definition's
; `declarator` field, so a function-pointer *parameter* (which lives under the
; `parameters` field) is never mistaken for a declaration. The extract engine
; then ascends the `declarator`-field chain to the `function_definition`, so
; metrics, span, and the body scope a call resolves into see the whole
; definition.
(function_definition
  declarator: [
    (function_declarator
      declarator: (identifier) @symbol.function)
    (pointer_declarator
      declarator: (function_declarator
        declarator: (identifier) @symbol.function))
    (pointer_declarator
      declarator: (pointer_declarator
        declarator: (function_declarator
          declarator: (identifier) @symbol.function)))])

; A function a test macro defines (S-583, CR-185, FR-EX-06): libuv's
; `TEST_IMPL(name) { … }` is a `function_definition` whose `type` is the macro
; and whose declarator is the parenthesised `(name)` — no `function_declarator`,
; so the pattern above never sees it and its calls were attributed to the file.
; The macro's argument names the function; `extract::testmarker::c_test_macro`
; marks it a test by the same macro list (`C_TEST_MACROS`, kept in step with this
; predicate). An exact list, never any `NAME(x) { … }` — a production macro of
; that shape is not a function here. The `TEST name(void) { … }` form (greatest)
; has an ordinary declarator and matches the pattern above. A two-argument
; `TEST(group, name) { … }` parses as a statement and a bare block, not a
; function, so it is out of reach. The pattern is not anchored to the file scope
; (libuv's cases sit inside `#ifdef` blocks), so a `TEST(x) { … }` written as a
; statement in a function body is read the same way.
(function_definition
  type: (type_identifier) @_macro
  declarator: (parenthesized_declarator
    (identifier) @symbol.function)
  (#any-of? @_macro "TEST_IMPL" "TEST" "TEST_CASE"))

; `typedef NAME` — a type alias (a name, not a class-like container).
(type_definition
  declarator: (type_identifier) @symbol.type_alias)

; A named `struct` or `union` with a body (S-582, FR-EX-31). The body is
; required: `struct X *p` and `struct X;` name a struct, they do not declare one,
; so redis's `struct redisCommand` is one Struct, not one per mention.
(struct_specifier
  name: (type_identifier) @symbol.struct
  body: (field_declaration_list))
(union_specifier
  name: (type_identifier) @symbol.struct
  body: (field_declaration_list))

; A member of a struct or union body, through every declarator C writes one
; with (S-582, FR-EX-31): `T x`, `T *p`, `T **pp`, `T a[N]`, `T a[N][M]`,
; `T *a[N]`, the function pointer `R (*f)(…)` — `R` a pointer too
; (`void *(*dup)(void *)`), and an array of them or a pointer to one (`(*f[N])`,
; `(**f)`) — and a bit field. Each
; declarator of `int a, *b;` is a member of its own, so the field-less child
; pattern sees them all (`@symbol.names`) — a `declarator:` field pattern would
; see the first only. A `field_declaration` is never anything but a member, so
; a function declarator a macro tore into one (`T f(…);`) is no field here.
; `@member.of`: a member no declaration contains — one recovery stranded
; outside every body, `UV_EXTERN uv_handle_type` read as a field — is dropped
; (`extract::member`).
(field_declaration
  [(field_identifier) @symbol.field @symbol.names @member.of
   (pointer_declarator
     declarator: [(field_identifier) @symbol.field @symbol.names @member.of
                  (pointer_declarator declarator: (field_identifier) @symbol.field @symbol.names @member.of)
                  (array_declarator declarator: (field_identifier) @symbol.field @symbol.names @member.of)])
   (array_declarator
     declarator: [(field_identifier) @symbol.field @symbol.names @member.of
                  (array_declarator declarator: (field_identifier) @symbol.field @symbol.names @member.of)])
   (function_declarator
     declarator: (parenthesized_declarator
       (pointer_declarator
         declarator: [(field_identifier) @symbol.field @symbol.names @member.of
                      (pointer_declarator declarator: (field_identifier) @symbol.field @symbol.names @member.of)
                      (array_declarator declarator: (field_identifier) @symbol.field @symbol.names @member.of)])))
   (pointer_declarator declarator: (function_declarator
       declarator: (parenthesized_declarator
         (pointer_declarator
           declarator: [(field_identifier) @symbol.field @symbol.names @member.of
                        (pointer_declarator declarator: (field_identifier) @symbol.field @symbol.names @member.of)
                        (array_declarator declarator: (field_identifier) @symbol.field @symbol.names @member.of)]))))
   (pointer_declarator declarator: (pointer_declarator declarator: (function_declarator
       declarator: (parenthesized_declarator
         (pointer_declarator
           declarator: [(field_identifier) @symbol.field @symbol.names @member.of
                        (pointer_declarator declarator: (field_identifier) @symbol.field @symbol.names @member.of)
                        (array_declarator declarator: (field_identifier) @symbol.field @symbol.names @member.of)])))))])

; A struct or union head stranded in an ERROR region (FR-EX-30, S-582). A
; header the grammar cannot read whole — libuv's `uv.h`, whose
; `struct uv_handle_s { UV_HANDLE_FIELDS };` is a macro the grammar never
; expands — leaves every later `struct uv_loop_s {` as loose tokens, its
; members loose beside them. The head is captured only when an opening brace
; follows the name — a definition, not a mention — and is kept only when the
; source writes `struct` or `union` before it (`extract::member` reads that
; off the text: recovery does not always leave the keyword a sibling, and a
; pattern naming it as one misses heads). It is the name itself (the engine
; never lifts into the ERROR) and, by `@member.torn`, owns the member
; declarations after it up to its closing brace.
(ERROR
  [(identifier) (type_identifier)] @symbol.struct @member.torn . "{")

; A named `enum` with a body — `enum mode m;` names one, it does not declare
; it (S-582). NodeKind::Enum is outside the Class ∪ Struct container set.
(enum_specifier
  name: (type_identifier) @symbol.enum
  body: (enumerator_list))

; Enumerators — file-scope named compile-time constants.
(enumerator
  name: (identifier) @symbol.constant)

; Object-style and function-style preprocessor macros.
(preproc_def
  name: (identifier) @symbol.macro)
(preproc_function_def
  name: (identifier) @symbol.macro)

; File-scope object declarations (globals). Anchored to `translation_unit` so a
; local variable inside a function body is never mistaken for a file-scope
; symbol. Covers the bare (`int y;`), initialised (`int x = 0;`), and pointer
; (`char *s;`, `int *p = 0;`) shapes.
(translation_unit
  (declaration
    declarator: [
      (identifier) @symbol.variable
      (pointer_declarator
        declarator: (identifier) @symbol.variable)
      (init_declarator
        declarator: (identifier) @symbol.variable)
      (init_declarator
        declarator: (pointer_declarator
          declarator: (identifier) @symbol.variable))]))

; A file-scope function pointer (`int (*hook)(void);`, `= 0` or not; S-584): a
; variable a call goes through, so the link unit never binds `hook()` to a
; function of that name in another file.
(translation_unit
  (declaration
    declarator: [
      (function_declarator
        declarator: (parenthesized_declarator
          (pointer_declarator
            declarator: (identifier) @symbol.variable)))
      (init_declarator
        declarator: (function_declarator
          declarator: (parenthesized_declarator
            (pointer_declarator
              declarator: (identifier) @symbol.variable))))]))

; ── Parameter range (S-591, CR-190, FR-EX-32) ────────────────────────────────
; The `@arity.*` vocabulary the extraction engine reads (`extract::arity`): a
; callable's parameter list, and each parameter as required, optional (a
; default value raises only the maximum), variadic (an unbounded maximum), a
; receiver (not counted) or no parameter at all (skip). A list child no capture
; covers, or one captured `@arity.unknown`, records the range unknown rather
; than miscounting it. Captures never name a declaration, so every symbol and
; node is unchanged.
; `...` is variadic and `(void)` declares none. An empty `()` declares
; unspecified parameters (before C23), so its range is unknown.
(function_declarator
  parameters: (parameter_list) @arity.parameters)
(parameter_list (parameter_declaration) @arity.required)
(parameter_list (variadic_parameter) @arity.variadic)
(parameter_list
  (parameter_declaration type: (primitive_type) @_void !declarator) @arity.skip
  (#eq? @_void "void"))
(parameter_list "(" . ")") @arity.unknown
