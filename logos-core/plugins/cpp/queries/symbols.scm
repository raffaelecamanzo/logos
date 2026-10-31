; C++ symbol-extraction query (S-058, capability = "symbols").
;
; Capture names map to NodeKind via NodeKind::as_str (extract engine). Compiled
; against the built C++ Language on its first use, or at load as an
; on-disk override; drift fails naming this file (FR-PL-02, CR-197).
; Droppable on disk at `.logos/plugins/cpp/queries/symbols.scm` (FR-PL-04,
; UAT-PL-03).
;
; The C family nests a function's name inside a `function_declarator` whose body
; is a sibling, so the engine lifts a captured declarator name to its
; body-bearing definition (`extract::lift_to_declaration`) — the captured node is
; still the leaf identifier, so the symbol name stays correct. The lift stops
; below an ERROR node (FR-EX-30): a declarator stranded in a parse-error region
; keeps its own span instead of the region's.
;
; Out-of-line member definitions (`void Widget::set() {…}`, a
; `qualified_identifier` declarator) are captured with their qualified name as
; `@decl.scope` (S-577, FR-EX-29, `extract::definition`): one whose class this
; file declares merges into the in-class prototype — one node, the prototype's
; symbol, the definition's body — so no second same-named node makes a
; `Widget::set` reference ambiguous (NFR-RA-05). One whose class another file
; declares (the header/source split) is a method of its own, its symbol naming
; the class (`widget.cpp/Widget#set().`). A prototype (`@decl.prototype`) and
; a bodied free function of one scope, name and parameters pair: both nodes
; stay, the prototype binds nothing and links to the definition. Constructors
; are captured by the free-function pattern (their declarator is a plain
; identifier) and, like template members, recorded as methods through
; `@decl.method`. Destructors and operators are not captured.

; A namespace is a module-like container (NodeKind::Module). Anonymous namespaces
; carry no `name` field and are not captured (their members are internal-linkage
; and never roots — see the cpp-external-linkage export rule).
(namespace_definition
  name: (namespace_identifier) @symbol.module)

(class_specifier
  name: (type_identifier) @symbol.class)

; A struct, union or enum is declared only with its body (S-582, FR-EX-31):
; `struct X *p` and `struct X;` name one, so they create no node.
(struct_specifier
  name: (type_identifier) @symbol.struct
  body: (field_declaration_list))

; A class/struct head stranded in an ERROR region (FR-EX-30, S-578). A macro the
; grammar cannot see (json's `JSON_HEDLEY_NON_NULL(3)` before a constructor)
; makes error recovery tear the `class_specifier` apart, leaving `class`, the
; name and the base clause loose in an ERROR node, so the class vanished. The
; head is captured only when the name is followed (past an optional `final`) by
; a base clause or the opening brace: it is a definition, not a stray keyword.
; The engine never lifts into the ERROR, so the node is the name itself — its
; body is not claimed, and its members stay with the enclosing scope.
(ERROR
  "class" . (type_identifier) @symbol.class . (virtual_specifier)? .
  [(base_class_clause) "{"])

(ERROR
  "struct" . (type_identifier) @symbol.struct . (virtual_specifier)? .
  [(base_class_clause) "{"])

; A C++ `union` is a named aggregate — mapped to the struct kind (it has fields,
; no method-contract semantics).
(union_specifier
  name: (type_identifier) @symbol.struct
  body: (field_declaration_list))

(enum_specifier
  name: (type_identifier) @symbol.enum
  body: (enumerator_list))

; A free function — both definitions (in `.cpp`) and prototypes (in `.h`): the
; declarator identifier is the name. The engine lifts the captured declarator to
; its owner — a body-bearing `function_definition` (so per-function metrics see
; the body) or a `declaration` for a prototype. GoogleTest `TEST(Suite, Name) {…}`
; macros also match here (they parse as return-type-less function_definitions
; named for the macro) and carry test evidence via the cpp-test-macros
; convention. A constructor and a template member in a class body match here
; too, and `@decl.method` below records them as methods. Member functions use a
; `field_identifier` declarator (next pattern), and out-of-line definitions a
; `qualified_identifier` (the pattern after), so neither is captured here.
(function_declarator
  declarator: (identifier) @symbol.function)

; A member function — both in-class definitions and prototypes: the declarator's
; `field_identifier` is the name. It is a method only in a class body, at any
; depth of `#if`/`#elif`/`#else` nesting (`@decl.member`, S-582, FR-EX-31): one
; outside every class body is a function. A header whose macros the grammar cannot see
; (`UV_EXTERN int f(…);`) tears into `field_declaration`s outside any class body —
; each was a Method of no class, 277 in libuv's `uv.h`; each is the free
; function it declares. Function and method share one descriptor, so the symbol
; is the same either way.
(function_declarator
  declarator: (field_identifier) @symbol.function @decl.member)

; An out-of-line definition, `R Cls::m(…) {…}`, its name up to three scopes
; deep (`ns::Outer::Inner::m`) and through a pointer, pointer-to-pointer or
; reference return. The whole qualified name is the `@decl.scope`; the engine
; merges the definition into the prototype of the class it names (S-577, see
; the header). A
; destructor (`Cls::~Cls`) or an operator is not named by an `identifier`, so it
; is not captured — nor is its in-class declaration — and neither is a `::f`
; with no scope.
(function_definition
  declarator: [
    (function_declarator
      declarator: (qualified_identifier
        scope: (_)
        name: [
          (identifier) @symbol.method
          (qualified_identifier
            name: [
              (identifier) @symbol.method
              (qualified_identifier name: (identifier) @symbol.method)])]) @decl.scope)
    (pointer_declarator
      declarator: (function_declarator
        declarator: (qualified_identifier
          scope: (_)
          name: [
            (identifier) @symbol.method
            (qualified_identifier
              name: [
                (identifier) @symbol.method
                (qualified_identifier name: (identifier) @symbol.method)])]) @decl.scope))
    (pointer_declarator
      declarator: (pointer_declarator
        declarator: (function_declarator
          declarator: (qualified_identifier
            scope: (_)
            name: [
              (identifier) @symbol.method
              (qualified_identifier
                name: [
                  (identifier) @symbol.method
                  (qualified_identifier name: (identifier) @symbol.method)])]) @decl.scope)))
    (reference_declarator
      (function_declarator
        declarator: (qualified_identifier
          scope: (_)
          name: [
            (identifier) @symbol.method
            (qualified_identifier
              name: [
                (identifier) @symbol.method
                (qualified_identifier name: (identifier) @symbol.method)])]) @decl.scope))])

; A test macro names its test by its arguments (S-580, `extract::test_name`):
; GoogleTest's `TEST(Suite, Name) {…}` is `Suite.Name`, as the runner prints it,
; and Catch2's `TEST_CASE("name", "[tag]") {…}` / `SCENARIO("name") {…}` is its
; string — doctest's decorated `TEST_CASE("name" * doctest::skip())` too.
; GoogleTest's macro is a function the pattern above already names `TEST`; this
; pattern renames it. Catch2's parses as a call statement and a block beside it,
; so the block is the declaration — the node that holds the body's references —
; and the statement before it its head. The keywords are those of the
; `cpp-test-macros` convention (`extract/testmarker.rs`, `cpp_test_macro`), which
; `cpp_test_names.rs` pins together; a macro whose arguments are not these shapes
; keeps the name it had.
(function_definition
  declarator: (function_declarator
    declarator: (identifier) @_macro
    parameters: (parameter_list
      . (parameter_declaration !declarator type: (type_identifier) @test.suite)
      . (parameter_declaration !declarator type: (type_identifier) @test.name) .))
  (#any-of? @_macro "TEST" "TEST_F" "TEST_P" "TYPED_TEST" "TYPED_TEST_P")) @test.decl

((expression_statement
   (call_expression
     function: (identifier) @_macro
     arguments: (argument_list
       . [(string_literal) @test.name
          (binary_expression left: (string_literal) @test.name)
          (binary_expression left: (binary_expression left: (string_literal) @test.name))]))) @test.head
 . (compound_statement) @symbol.function @test.decl
 (#any-of? @_macro "TEST_CASE" "SCENARIO"))

; A declaration written in a class body: a constructor or template member the
; free-function pattern names is a method (S-577). Function and method share
; one descriptor, so the symbol is unchanged.
(field_declaration_list
  [(declaration) (function_definition)] @decl.method)
(field_declaration_list
  (template_declaration [(declaration) (function_definition)] @decl.method))
; …and in an `#if`/`#ifdef` block of a class body, either branch (ccache's
; `DirEntry` constructor under `#ifndef _WIN32`).
(field_declaration_list
  [(preproc_if [(declaration) @decl.method (function_definition) @decl.method
     (template_declaration [(declaration) (function_definition)] @decl.method)])
   (preproc_ifdef [(declaration) @decl.method (function_definition) @decl.method
     (template_declaration [(declaration) (function_definition)] @decl.method)])
   (preproc_if alternative: (_ [(declaration) @decl.method (function_definition) @decl.method
     (template_declaration [(declaration) (function_definition)] @decl.method)]))
   (preproc_ifdef alternative: (_ [(declaration) @decl.method (function_definition) @decl.method
     (template_declaration [(declaration) (function_definition)] @decl.method)]))])

; A prototype: a bodiless function declaration, in a class body or out of one.
; A definition of its scope completes it — an out-of-line member merges into
; it, a free function of its name and parameters pairs with it (S-577).
(declaration
  declarator: [
    (function_declarator)
    (pointer_declarator declarator: (function_declarator))
    (pointer_declarator declarator: (pointer_declarator declarator: (function_declarator)))
    (reference_declarator (function_declarator))]) @decl.prototype
(field_declaration
  declarator: [
    (function_declarator)
    (pointer_declarator declarator: (function_declarator))
    (pointer_declarator declarator: (pointer_declarator declarator: (function_declarator)))
    (reference_declarator (function_declarator))]) @decl.prototype

; A data member, through every declarator one is written with (S-582,
; FR-EX-31): `T x_`, `T *p`, `T **pp`, `T a[N]`, `T a[N][M]`, `T *a[N]`, the
; function pointer `R (*f)(…)` — `R` a pointer too (`void *(*dup)(void *)`), and
; an array of them or a pointer to one (`(*f[N])`, `(**f)`) — and a bit field. Each declarator of
; `int a, *b;` is a member of its own, so the field-less child pattern sees
; them all (`@symbol.names`) — a `declarator:` field pattern would see the
; first only. A member *function* wraps its name in a `function_declarator`
; directly, never a bare or pointer `field_identifier`, so this never captures
; a method prototype as a field. A member no captured declaration contains —
; of a template specialisation (`struct Box<int> {…}`) or a nested class
; defined out of line (`struct Widget::Impl {…}`) — stays a field of its scope.
(field_declaration
  [(field_identifier) @symbol.field @symbol.names
   (pointer_declarator
     declarator: [(field_identifier) @symbol.field @symbol.names
                  (pointer_declarator declarator: (field_identifier) @symbol.field @symbol.names)
                  (array_declarator declarator: (field_identifier) @symbol.field @symbol.names)])
   (array_declarator
     declarator: [(field_identifier) @symbol.field @symbol.names
                  (array_declarator declarator: (field_identifier) @symbol.field @symbol.names)])
   (function_declarator
     declarator: (parenthesized_declarator
       (pointer_declarator
         declarator: [(field_identifier) @symbol.field @symbol.names
                      (pointer_declarator declarator: (field_identifier) @symbol.field @symbol.names)
                      (array_declarator declarator: (field_identifier) @symbol.field @symbol.names)])))
   (pointer_declarator declarator: (function_declarator
       declarator: (parenthesized_declarator
         (pointer_declarator
           declarator: [(field_identifier) @symbol.field @symbol.names
                        (pointer_declarator declarator: (field_identifier) @symbol.field @symbol.names)
                        (array_declarator declarator: (field_identifier) @symbol.field @symbol.names)]))))
   (pointer_declarator declarator: (pointer_declarator declarator: (function_declarator
       declarator: (parenthesized_declarator
         (pointer_declarator
           declarator: [(field_identifier) @symbol.field @symbol.names
                        (pointer_declarator declarator: (field_identifier) @symbol.field @symbol.names)
                        (array_declarator declarator: (field_identifier) @symbol.field @symbol.names)])))))])

; `typedef int Integer;` and `using Integer = int;` — both type aliases.
(type_definition
  declarator: (type_identifier) @symbol.type_alias)

(alias_declaration
  name: (type_identifier) @symbol.type_alias)

; ── Parameter range (S-591, CR-190, FR-EX-32) ────────────────────────────────
; The `@arity.*` vocabulary the extraction engine reads (`extract::arity`): a
; callable's parameter list, and each parameter as required, optional (a
; default value raises only the maximum), variadic (an unbounded maximum), a
; receiver (not counted) or no parameter at all (skip). A list child no capture
; covers, or one captured `@arity.unknown`, records the range unknown rather
; than miscounting it. Captures never name a declaration, so every symbol and
; node is unchanged.
; A parameter with a default value is optional; `...` and a parameter pack
; (`Ts... xs`) are variadic; `(void)` declares none.
;
; Defaults may be written on a declaration and not on the definition
; (`int f(int a, int b = 2);` in a header, `int f(int a, int b) {…}` in the
; source), so a definition outside a class body records no range: its own list
; can claim a minimum a legal call contradicts. A declaration, and a definition
; in a class body (its only declaration), record theirs.
(declaration
  declarator: [(function_declarator parameters: (parameter_list) @arity.parameters)
    (pointer_declarator declarator: (function_declarator parameters: (parameter_list) @arity.parameters))
    (reference_declarator (function_declarator parameters: (parameter_list) @arity.parameters))])
(field_declaration_list
  (field_declaration
    declarator: [(function_declarator parameters: (parameter_list) @arity.parameters)
    (pointer_declarator declarator: (function_declarator parameters: (parameter_list) @arity.parameters))
    (reference_declarator (function_declarator parameters: (parameter_list) @arity.parameters))]))
(field_declaration_list
  (function_definition
    declarator: [(function_declarator parameters: (parameter_list) @arity.parameters)
    (pointer_declarator declarator: (function_declarator parameters: (parameter_list) @arity.parameters))
    (reference_declarator (function_declarator parameters: (parameter_list) @arity.parameters))]))
(field_declaration_list
  (template_declaration
    (function_definition
      declarator: [(function_declarator parameters: (parameter_list) @arity.parameters)
    (pointer_declarator declarator: (function_declarator parameters: (parameter_list) @arity.parameters))
    (reference_declarator (function_declarator parameters: (parameter_list) @arity.parameters))])))
(parameter_list (parameter_declaration) @arity.required)
(parameter_list (optional_parameter_declaration) @arity.optional)
(parameter_list (variadic_parameter_declaration) @arity.variadic)
(parameter_list "..." @arity.variadic)
(parameter_list
  (parameter_declaration type: (primitive_type) @_void !declarator) @arity.skip
  (#eq? @_void "void"))
