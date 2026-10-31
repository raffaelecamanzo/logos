; C++ reference-extraction query (S-058, capability = "references").
;
;   @ref.method — a call's name (`free()`, `obj.method()`, `obj->method()`);
;                 a Method-form row that binds by its receiver's SHAPE, recorded
;                 by the `@ref.receiver.*` markers below (S-514, S-516, CR-169).
;                 A bare `free()` outside a class body becomes the Path-form
;                 free call the receiver pass records.
;   @ref.call   — a qualified call's whole name (`ns::func()`,
;                 `Type::static_method()`, `ns::f<T>()`), recorded in Path form
;                 (S-579, FR-RS-38): bound through the caller's enclosing
;                 namespaces and class, its file's `using` declarations and
;                 directives, and the declarations its `#include`s make
;                 visible — exactly one, or nothing.
;   @ref.import — an `#include` header (S-579): `"x.h"` read against the
;                 including file's directory, then the include roots; `<a/b.h>`
;                 against the include roots alone (`plugin.toml`). A header no
;                 root holds (`<vector>`) stays external. Its row binds the
;                 included file and names nothing, so it carries no alias.
;                 Also a `using` (`@ref.import.using`): `using ns::f;` is a Path
;                 row aliased `f`, `using namespace ns;` (`@ref.import.asterisk`)
;                 a Glob row — both read as member paths.
;   @ref.extends — a base class (`class A : public B`, `ns::B`, `B<T>`;
;                 S-579): an `Extends` row of the class, bound through the same
;                 scopes and includes as a qualified call.
;   @ref.access — an own-field access (`this->x`): a method reading a field of
;                 its own class (CR-005, FR-EX-08). The receiver-anchored pattern
;                 restricts it to `this->`, and the `this->f()` call it also
;                 matches is dropped (`@ref.access.selection`, S-580); the
;                 enclosing method Contains both endpoints, so resolution
;                 binds an exactly-one Field candidate to an `Accesses` edge
;                 (Method → Field) — the bound LCOM4 input.
;   @ref.access.selection — the `this->x` selection a `@ref.access` reads
;                 (S-580): `collect_refs` drops the row when an argument list
;                 applies it, which is a call (`this->f()`), not a field read.
;   @ref.access.bare — a bare name in a value position (`m_x`, the receiver of
;                 `m_log.add(x)`): an access when the method's class declares a
;                 field of that name and the method declares no local of it
;                 (S-580, FR-EX-22). See "Bare field access" below.
;   @ref.access.shadow — a name a method declares (a parameter, a local, a
;                 structured binding, a loop, catch or lambda variable): it
;                 records no row; it drops the `@ref.access.bare` rows of that
;                 method headed by its name.
;
; Receiver-shape markers (S-516, FR-EX-13, FR-RS-12) — they record no row of
; their own; each names the receiver of the `@ref.method` that shares its parent
; node (the `field_expression` / `qualified_identifier`, or for a bare call the
; `call_expression`):
;
;   @ref.receiver.self     — `this->method()`, `(*this).method()`: binds among
;                            the enclosing class's own members only.
;   @ref.receiver.other    — every other receiver (`obj.method()`,
;                            `ptr->method()`): never bound through the
;                            caller's scope (`no-receiver-evidence`). C++ has
;                            no `super` keyword; a base-class qualifier
;                            (`Base::method()`) is the same syntax as a
;                            namespace one, a Path-form `@ref.call` (S-579).
;   @ref.receiver.implicit — `method()`, no receiver: under this plugin's
;                            `implicit_receiver = "self"` a call on the current
;                            instance inside a class or struct body, a free call
;                            outside one (a free function, or an out-of-line
;                            `Type::method` definition, which no declaration
;                            here encloses).
;
; Receiver-typing markers (S-581) — `variable`, `pointee`, `binding`, `proof`:
; see "Receiver typing" below.
;
; Droppable on disk at `.logos/plugins/cpp/queries/references.scm`.
;
; Deliberately NOT captured: `new T()` construction; template instantiation
; and macro-expanded calls, and an `#include` a macro names (`#include
; HEADER`) — no preprocessor runs (CR-184 §3.3). Unbindable constructs yield
; missing edges, never wrong ones (NFR-RA-05).

; A free / unqualified call: `free_call()`.
(call_expression
  function: (identifier) @ref.method @ref.receiver.implicit)

; A member call: `obj.method()` / `obj->method()` (both are `field_expression`).
(call_expression
  function: (field_expression
    field: (field_identifier) @ref.method))

; A qualified call: `ns::func()` / `Type::static_method()` / `ns::f<T>()` — the
; whole qualified name, a Path-form row (S-579). A nested `a::b::f()` is one
; `qualified_identifier` whose name is another; the outermost is captured.
(call_expression
  function: (qualified_identifier
    name: [(identifier) (template_function) (qualified_identifier)]) @ref.call)

; The receiver of a member call, and the scope of a qualified one.
(field_expression
  argument: (this) @ref.receiver.self)

(field_expression
  argument: (parenthesized_expression
    (pointer_expression
      argument: (this))) @ref.receiver.self)

(field_expression
  argument: (_) @ref.receiver.other)

; Receiver typing (S-581, CR-184, FR-RS-39, FR-RS-10) — the scope-precise
; markers Rust's typing speaks (`extract::receiver`), recording no row of their
; own. Where the one binding of a member call's receiver in scope at the call
; proves its type `T`, the Method row is retyped to the Path-form `T::m`, which
; binds as a written qualified call does (S-579): through the caller's
; namespaces, its `using`s and its includes, to `T`'s own `m` or a proven
; base's. Anything unproven keeps its `other` row (`no-receiver-evidence`):
;
;   @ref.receiver.variable — `x.m()`: a one-name receiver read through `.`,
;                            proven by a value or reference declaration.
;   @ref.receiver.pointee  — `p->m()`: one read through `->`, proven by a
;                            pointer declaration alone. `x->m()` on a value is
;                            its `operator->`, and `p.reset()` on a smart
;                            pointer the pointer's own member: neither is typed.
;   @ref.receiver.binding  — each name a declaration binds, and where:
;                            `.scope` the function, lambda, `catch`, `for`,
;                            range-`for`, `if`, `while` or `switch` a parameter
;                            or condition variable belongs to (the parameter
;                            list sits in the declarator, never around the
;                            body); `.after` a local's block after it;
;                            `.member` a data member's class, struct or union —
;                            only where that type is the call's own (an inline
;                            method of it, a lambda in one).
;   @ref.receiver.hides    — every name a declarator declares; one no binding
;                            states hides its name across its function
;                            (`@ref.receiver.hiding_scope`), proving nothing:
;                            a `case` local (in scope in every later label),
;                            a labelled one, one under nested `#if`s.
;   @ref.receiver.proof    — the binding's type: `.type` a declared `T`, `T&`,
;                            `const T&`, `T&&` (`ns::T`, and `T<A>` reads `T`);
;                            `.pointer` a declared `T*`, `std::unique_ptr<T>`
;                            or `std::shared_ptr<T>`, the companion naming `T`.
;                            A binding with no proof — `auto`, an array, a
;                            structured binding, a pointer to a pointer, a
;                            function pointer, a primitive — proves nothing,
;                            and hides every other binding of its name.
;
; Stated ceilings: two bindings of a name in scope prove nothing, so a
; parameter or local that shadows a data member refuses the call rather than
; picking the nearer one; a member a base declares is not in scope, and
; neither is any member in an out-of-line `T::f` definition, whose class this
; file may not hold; a namespace-scope variable binds nowhere (an out-of-line
; method's member of its name would hide it); `this->x.m()`, `(*p).m()`, a
; chain (`ctx.config.m()`), a template parameter `T` and `auto` prove nothing;
; and a declaration no binding scopes (a `case` local, a labelled one, one under
; nested `#if`s) hides its name across its function and types no call.
(call_expression
  function: (field_expression
    argument: (identifier) @ref.receiver.variable
    operator: "."))
(call_expression
  function: (field_expression
    argument: (identifier) @ref.receiver.pointee
    operator: "->"))

; Parameters: of a function (its declarator under a returned `*`, `**` or `&`
; included), of a lambda, of a `catch`; a lambda's init-captures.
(function_definition
  declarator: (function_declarator
    parameters: (parameter_list (_ declarator: (_) @ref.receiver.binding)))) @ref.receiver.binding.scope
(function_definition
  declarator: (pointer_declarator
    declarator: (function_declarator
      parameters: (parameter_list (_ declarator: (_) @ref.receiver.binding))))) @ref.receiver.binding.scope
(function_definition
  declarator: (pointer_declarator
    declarator: (pointer_declarator
      declarator: (function_declarator
        parameters: (parameter_list (_ declarator: (_) @ref.receiver.binding)))))) @ref.receiver.binding.scope
(function_definition
  declarator: (reference_declarator
    (function_declarator
      parameters: (parameter_list (_ declarator: (_) @ref.receiver.binding))))) @ref.receiver.binding.scope
(lambda_expression
  declarator: (abstract_function_declarator
    parameters: (parameter_list (_ declarator: (_) @ref.receiver.binding)))) @ref.receiver.binding.scope
(lambda_expression
  captures: (lambda_capture_specifier
    (lambda_capture_initializer left: (identifier) @ref.receiver.binding))) @ref.receiver.binding.scope
(catch_clause
  parameters: (parameter_list (_ declarator: (_) @ref.receiver.binding))) @ref.receiver.binding.scope

; Locals, from their declaration to the end of their block — and,
; where an `#if` / `#ifdef` branch holds one, the whole block around it, so a
; name another branch declares too is two bindings and proves nothing. A
; namespace-scope declaration is none of these.
(compound_statement
  (declaration declarator: [
    (init_declarator declarator: (_) @ref.receiver.binding)
    [(identifier) (pointer_declarator) (reference_declarator) (array_declarator) (function_declarator)] @ref.receiver.binding]) @ref.receiver.binding.after)
(compound_statement
  [(preproc_if (declaration declarator: [(init_declarator declarator: (_) @ref.receiver.binding) (identifier) @ref.receiver.binding (pointer_declarator) @ref.receiver.binding (reference_declarator) @ref.receiver.binding])) (preproc_ifdef (declaration declarator: [(init_declarator declarator: (_) @ref.receiver.binding) (identifier) @ref.receiver.binding (pointer_declarator) @ref.receiver.binding (reference_declarator) @ref.receiver.binding]))]) @ref.receiver.binding.scope
(compound_statement
  [(preproc_if alternative: (_ (declaration declarator: [(init_declarator declarator: (_) @ref.receiver.binding) (identifier) @ref.receiver.binding (pointer_declarator) @ref.receiver.binding (reference_declarator) @ref.receiver.binding]))) (preproc_ifdef alternative: (_ (declaration declarator: [(init_declarator declarator: (_) @ref.receiver.binding) (identifier) @ref.receiver.binding (pointer_declarator) @ref.receiver.binding (reference_declarator) @ref.receiver.binding])))]) @ref.receiver.binding.scope

; Variables a `for`, a range-`for` or a condition declares, for the whole
; statement.
(for_statement
  initializer: (declaration declarator: [(init_declarator declarator: (_) @ref.receiver.binding) (identifier) @ref.receiver.binding (pointer_declarator) @ref.receiver.binding (reference_declarator) @ref.receiver.binding])) @ref.receiver.binding.scope
(for_range_loop
  declarator: (_) @ref.receiver.binding) @ref.receiver.binding.scope
(_
  condition: (condition_clause
    initializer: (init_statement (declaration declarator: [(init_declarator declarator: (_) @ref.receiver.binding) (identifier) @ref.receiver.binding (pointer_declarator) @ref.receiver.binding (reference_declarator) @ref.receiver.binding])))) @ref.receiver.binding.scope
(_
  condition: (condition_clause
    value: (declaration declarator: (_) @ref.receiver.binding))) @ref.receiver.binding.scope

; Data members of a class, a struct, a union.
; Only a class, a struct and a union have a `field_declaration_list` body.
(_
  body: (field_declaration_list
    (field_declaration declarator: [(field_identifier) (pointer_declarator) (reference_declarator) (array_declarator)] @ref.receiver.binding))) @ref.receiver.binding.member

; A declared type, wherever the declaration sits — a parameter, a local, a
; condition or loop variable, a data member; only a binding above reads it. A
; value or a reference proves `.type`; a pointer, and a `std::unique_ptr` or
; `std::shared_ptr` of one type, prove `.pointer` — so a smart pointer is
; never a value's proof.
((_
  type: [(type_identifier) (qualified_identifier) (template_type)] @ref.receiver.proof.type
  declarator: [
    (identifier) @ref.receiver.proof
    (field_identifier) @ref.receiver.proof
    (reference_declarator [(identifier) (field_identifier)] @ref.receiver.proof)
    (init_declarator declarator: [
      (identifier) @ref.receiver.proof
      (reference_declarator (identifier) @ref.receiver.proof)])])
  (#not-match? @ref.receiver.proof.type "^std::(unique|shared)_ptr\\s*<"))
(_
  type: [(type_identifier) (qualified_identifier) (template_type)] @ref.receiver.proof.pointer
  declarator: [
    (pointer_declarator declarator: [(identifier) (field_identifier)] @ref.receiver.proof)
    (init_declarator declarator: (pointer_declarator declarator: (identifier) @ref.receiver.proof))])
((_
  type: (qualified_identifier
    scope: (namespace_identifier) @_std
    name: (template_type
      name: (type_identifier) @_smart
      arguments: (template_argument_list
        .
        (type_descriptor
          !declarator
          type: [(type_identifier) (qualified_identifier) (template_type)] @ref.receiver.proof.pointer)
        .)))
  declarator: [
    (identifier) @ref.receiver.proof
    (field_identifier) @ref.receiver.proof
    (reference_declarator [(identifier) (field_identifier)] @ref.receiver.proof)
    (init_declarator declarator: [
      (identifier) @ref.receiver.proof
      (reference_declarator (identifier) @ref.receiver.proof)])])
  (#eq? @_std "std")
  (#any-of? @_smart "unique_ptr" "shared_ptr"))


; An own-field access through `this->` — the bound LCOM4 / Accesses input. The
; same shape spells a call, `this->f()`: `collect_refs` drops the row when an
; argument list applies the selection (`@ref.access.selection`), and the
; method rows above record the call (S-580).
(field_expression
  argument: (this)
  field: (field_identifier) @ref.access) @ref.access.selection

; Bare field access (S-580, FR-EX-22). `m_x` read, written or used as a receiver
; (`m_log.add(x)`) inside a method of the class that declares it, with no
; `this->` written. Only the positions a VALUE is spelled in are listed, so a
; name in a TYPE or DECLARATION position is never captured, and the bare name
; of a CALL (`m_f(x)`) is no value — it is the method rows' concern.
; `collect_refs` records a row only where the method sits in a class this file
; declares that itself declares a field of that name (`bare_access`), and drops
; it where the same method declares a parameter or local of the name
; (`@ref.access.shadow` below); the binder then proves an exactly-one Field in
; that class (NFR-RA-05). A member a class inherits, or one an out-of-line
; definition reads in a file that does not declare the class (the header and
; source split), is not seen: it stays unrecorded. `T t(m_x);` parses as a
; function declaration, so its `m_x` is a type name and records nothing.
[
  (argument_list (identifier) @ref.access.bare)
  (field_expression argument: (identifier) @ref.access.bare)
  (binary_expression left: (identifier) @ref.access.bare)
  (binary_expression right: (identifier) @ref.access.bare)
  (unary_expression argument: (identifier) @ref.access.bare)
  (update_expression argument: (identifier) @ref.access.bare)
  (assignment_expression left: (identifier) @ref.access.bare)
  (assignment_expression right: (identifier) @ref.access.bare)
  (conditional_expression condition: (identifier) @ref.access.bare)
  (conditional_expression consequence: (identifier) @ref.access.bare)
  (conditional_expression alternative: (identifier) @ref.access.bare)
  (subscript_expression argument: (identifier) @ref.access.bare)
  (subscript_argument_list (identifier) @ref.access.bare)
  (pointer_expression argument: (identifier) @ref.access.bare)
  (parenthesized_expression (identifier) @ref.access.bare)
  (cast_expression value: (identifier) @ref.access.bare)
  (comma_expression left: (identifier) @ref.access.bare)
  (comma_expression right: (identifier) @ref.access.bare)
  (condition_clause value: (identifier) @ref.access.bare)
  (return_statement (identifier) @ref.access.bare)
  (throw_statement (identifier) @ref.access.bare)
  (init_declarator value: (identifier) @ref.access.bare)
  (declaration value: (identifier) @ref.access.bare)
  (initializer_list (identifier) @ref.access.bare)
  (for_range_loop right: (identifier) @ref.access.bare)
  (lambda_capture_initializer right: (identifier) @ref.access.bare)
]

; What a method declares hides a field of the name from it — and a receiver's
; outer binding of the name (below): a parameter, a local
; (an array, a pointer or a function pointer too, whatever the declarator wraps
; the name in), a structured binding, a range-for variable, a catch or lambda
; parameter, and a lambda's init-capture (`[m_x = 5]`). The name is the identifier
; a declarator holds directly, so one pattern per wrapper reads it at any depth.
(parameter_declaration declarator: (identifier) @ref.access.shadow @ref.receiver.hides)
(optional_parameter_declaration declarator: (identifier) @ref.access.shadow @ref.receiver.hides)
(variadic_parameter_declaration declarator: (variadic_declarator (identifier) @ref.access.shadow @ref.receiver.hides))
(declaration declarator: (identifier) @ref.access.shadow @ref.receiver.hides)
(init_declarator declarator: (identifier) @ref.access.shadow @ref.receiver.hides)
(for_range_loop declarator: (identifier) @ref.access.shadow @ref.receiver.hides)
(pointer_declarator declarator: (identifier) @ref.access.shadow @ref.receiver.hides)
(array_declarator declarator: (identifier) @ref.access.shadow @ref.receiver.hides)
(function_declarator declarator: (identifier) @ref.access.shadow @ref.receiver.hides)
(reference_declarator (identifier) @ref.access.shadow @ref.receiver.hides)
(parenthesized_declarator (identifier) @ref.access.shadow @ref.receiver.hides)
(structured_binding_declarator (identifier) @ref.access.shadow @ref.receiver.hides)
(lambda_capture_initializer left: (identifier) @ref.access.shadow @ref.receiver.hides)

; The same names are receiver-typing hiders (S-581, `@ref.receiver.hides`): one
; no `@ref.receiver.binding` above states — a labelled declaration, a `case`'s,
; one under nested `#if`s, a parameter of a declarator nesting not listed — is
; bound with no proof across the innermost function or lambda around it, so it
; hides every other binding of its name there rather than leaving an outer one
; the only binding in scope. A name in no function (a prototype's parameter, a
; namespace-scope variable) hides nothing.
(function_definition) @ref.receiver.hiding_scope
(lambda_expression) @ref.receiver.hiding_scope

; `#include "x.h"` / `#include <a/b.h>` (S-579). A macro-named header is
; neither node kind, so it records nothing.
(preproc_include
  path: [(string_literal) (system_lib_string)] @ref.import)

; `using ns::f;` and `using namespace ns;` (S-579): the name it brings into the
; file's scope, or the namespace whose members it does.
(using_declaration
  "namespace"? @ref.import.asterisk
  [(identifier) (qualified_identifier)] @ref.import @ref.import.using)

; A base class (S-579): `class Manifest : public Serializer`, `: ns::Base`,
; `: Base<T>`; each base of a list is its own match.
(base_class_clause
  [(type_identifier) (qualified_identifier) (template_type)] @ref.extends)

; ── Argument count (S-591, CR-190, FR-EX-32) ─────────────────────────────────
; The `@arity.*` vocabulary the extraction engine reads (`extract::arity`): every
; argument list, whose named children a call row counts, and the forms that
; make a count unknown. Captures record no row of their own, so every ledger
; target is unchanged.
; A pack expansion (`xs...`) spreads; a braced `new T{…}` is not counted.
(argument_list) @arity.arguments
(argument_list (parameter_pack_expansion) @arity.spread)
(new_expression
  arguments: (initializer_list) @arity.opaque)
