; C++ symbol-extraction query (S-058, capability = "symbols").
;
; Capture names map to NodeKind via NodeKind::as_str (extract engine). Compiled
; against the built C++ Language at load; fails fast naming this file on drift
; (FR-PL-02). Droppable on disk at `.logos/plugins/cpp/queries/symbols.scm`
; (FR-PL-04, UAT-PL-03).
;
; The C family nests a function's name inside a `function_declarator` whose body
; is a sibling, so the engine lifts a captured declarator name to its
; body-bearing definition (`extract::lift_to_declaration`) — the captured node is
; still the leaf identifier, so the symbol name stays correct.
;
; v1 policy: out-of-line member definitions (`void Widget::set() {…}`, a
; `qualified_identifier` declarator) are deliberately NOT captured — the member
; is already captured at its in-class declaration, and a second same-named node
; would make every `Widget::set` reference ambiguous under the binder's
; exactly-one-or-nothing rule (NFR-RA-05). Constructors/destructors are likewise
; left out (they share the class name; `new T()` references stay honestly
; unresolved), matching the Java plugin's constructor policy.

; A namespace is a module-like container (NodeKind::Module). Anonymous namespaces
; carry no `name` field and are not captured (their members are internal-linkage
; and never roots — see the cpp-external-linkage export rule).
(namespace_definition
  name: (namespace_identifier) @symbol.module)

(class_specifier
  name: (type_identifier) @symbol.class)

(struct_specifier
  name: (type_identifier) @symbol.struct)

; A C++ `union` is a named aggregate — mapped to the struct kind (it has fields,
; no method-contract semantics).
(union_specifier
  name: (type_identifier) @symbol.struct)

(enum_specifier
  name: (type_identifier) @symbol.enum)

; A free function — both definitions (in `.cpp`) and prototypes (in `.h`): the
; declarator identifier is the name. The engine lifts the captured declarator to
; its owner — a body-bearing `function_definition` (so per-function metrics see
; the body) or a `declaration` for a prototype. GoogleTest `TEST(Suite, Name) {…}`
; macros also match here (they parse as return-type-less function_definitions
; named for the macro) and carry test evidence via the cpp-test-macros
; convention. Member functions use a `field_identifier` declarator (next
; pattern), and out-of-line definitions a `qualified_identifier`, so neither is
; captured here.
(function_declarator
  declarator: (identifier) @symbol.function)

; A member function — both in-class definitions and prototypes: the declarator's
; `field_identifier` is the method name. Out-of-line definitions use a
; `qualified_identifier` declarator and are excluded (see the v1 policy above).
(function_declarator
  declarator: (field_identifier) @symbol.method)

; A data member (`int x_;`). A member *function* declaration wraps its name in a
; `function_declarator`, not a bare `field_identifier`, so this never captures a
; method prototype as a field.
(field_declaration
  declarator: (field_identifier) @symbol.field)

; `typedef int Integer;` and `using Integer = int;` — both type aliases.
(type_definition
  declarator: (type_identifier) @symbol.type_alias)

(alias_declaration
  name: (type_identifier) @symbol.type_alias)
