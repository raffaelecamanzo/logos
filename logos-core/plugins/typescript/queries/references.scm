; TypeScript reference-extraction query (S-015, capability = "references").
;
;   @ref.call   — a plain-identifier call (`f()`).
;   @ref.method — a member call (`obj.m()`, `app.get(...)`): the name alone,
;                 bound by its receiver's SHAPE (the markers below) — except a
;                 member call on a namespace import (S-440, below), recorded
;                 as the module-qualified path instead.
;   @ref.import — an import source string (`import x from "express"`), a
;                 CommonJS `require("...")` argument or a dynamic `import("...")`
;                 argument (S-534); the quoted text is unquoted and
;                 canonicalised by the PATH grammar the descriptor declares
;                 (S-439) into the `::`-joined ledger form feeding the binder
;                 and the framework candidacy gate (FR-FW-04).
;
; Droppable on disk at `.logos/plugins/typescript/queries/references.scm`.
;
; Named-import bindings (`import { Router } from './r'`), default imports
; (`import App from './app'`, S-534) and namespace imports (`import * as m from
; './m'`) are read from the `@ref.import` statement by the extraction engine
; itself (S-440), which records a call through one — `Router()`, `App()`,
; `m.f()` — qualified by its module (a default import as `<module>::default`).
; A dynamic `import('./x')` with a literal specifier is an import too (S-534);
; one whose specifier is a template or an expression is not.
;
;   @ref.export — an `export` statement at file level (S-534, FR-RS-19): the
;                 engine reads its shape. A re-export (`export * from 's'`,
;                 `export { a as b } from 's'`, `export { default } from 's'`)
;                 is an import of `s` marked exported, and a local export
;                 under another name (`export default App`, `export default
;                 function App`, `export { a as default }`) records the
;                 module's export alias — what the binder follows a name
;                 imported from this module through.

(call_expression
  function: (identifier) @ref.call)

(call_expression
  function: (member_expression
    property: (property_identifier) @ref.method))

;   Receiver markers (S-515, CR-169, FR-EX-13) — MARKERS, recording nothing on
;   their own. Each names the receiver of the member call above: its captured
;   node shares the `member_expression` the `@ref.method` name sits in, and
;   the call records that SHAPE, which the binder dispatches on (FR-RS-12).
;   `.js` sources parse with this grammar, so JavaScript is covered too.
;
;   @ref.receiver.self      — `this.m()`: binds among the enclosing class's own
;                             members only.
;   @ref.receiver.super     — `super.m()`: binds only through a proven
;                             `Extends` (S-535, below) — the nearest supertype
;                             declaring exactly one `m` — and never reaches the
;                             caller's own class.
;   @ref.receiver.other     — every receiver (`obj.m()`, `this.x.m()`, a chained
;                             call): never binds through the caller's scope —
;                             typed where the file proves the receiver's type
;                             ("Receiver typing" below). `self`/`super` outrank
;                             it.
;   @ref.receiver.anonymous — a body whose `this` is not the enclosing class's
;                             instance, so a `this.m()` inside is never that
;                             class's `m`: a class EXPRESSION's body (`return
;                             class { … }`, no class node — `other`, unless the
;                             body declares `m` itself), and the bodies that
;                             REBIND `this` — an object-literal method and a
;                             non-arrow `function` (`other`). An arrow function
;                             keeps the method's `this`, so it is not marked.
;
;   An unqualified call (`m()`) is never a call on the instance in TS/JS: it is
;   the free `@ref.call` above (`implicit_receiver` stays "none").
(call_expression
  function: (member_expression object: (this) @ref.receiver.self))
(call_expression
  function: (member_expression object: (super) @ref.receiver.super))
(call_expression
  function: (member_expression object: (_) @ref.receiver.other))
(class body: (class_body) @ref.receiver.anonymous)
(object (method_definition) @ref.receiver.anonymous)
[(function_expression) (function_declaration) (generator_function)
 (generator_function_declaration)] @ref.receiver.anonymous

;   Receiver typing (S-537, CR-173, FR-RS-21, FR-RS-10) — markers recording no
;   row of their own (`extract::receiver`). Where the file proves a member
;   call's receiver type `T`, its Method row is retyped to the Path-form `T::m`,
;   keeping its `other` shape — `T` qualified by the named import it came
;   through, as a type relation's is (S-535) — and bound among `T`'s members,
;   up its proven `Extends`, an abstract class's or an interface's bodyless
;   method included; anything unproven keeps its `other` row
;   (`no-receiver-evidence`).
;
;   A plain name, `svc.m()`, speaks Rust's binding vocabulary (S-587): it is
;   typed by the ONE binding of `svc` in scope at the call, when that binding
;   carries a proof. Two bindings in scope — a shadowing local, an inner
;   function's parameter, a module-level variable beside a parameter — prove
;   nothing, nor does a binding with no proof.
;
;   @ref.receiver.variable — `svc.m()`: the receiver name.
;   @ref.receiver.binding  — every name a declaration binds — a parameter (each
;                            name of a destructured one), an arrow function's
;                            lone parameter, a `catch` or `for … of`/`in`
;                            variable (`.scope`: the function, clause or loop),
;                            a `const`/`let` (`.after`: the rest of its block),
;                            a `var` (no companion: the whole callable).
;   @ref.receiver.proof    — a binding's proven type: an annotated parameter or
;                            variable (`.type`: `T`, `T<A>`), `const x = new
;                            T(…)` (`.literal`: `T`). A union, array, function,
;                            object-literal or qualified (`ns.T`) annotation is
;                            no proof.
;
;   `this.svc.m()` speaks Java's (S-467): the field `svc` the enclosing class
;   declares itself, read from the class's own field declarations — every one
;   of which this query states:
;
;   @ref.receiver.field    — `this.svc.m()` (and `this.#svc.m()`).
;   @ref.receiver.declared — a field: an annotated field or a constructor
;                            parameter property (`private readonly svc: T`,
;                            the `.field` companion: declared in the
;                            constructor's parameter list). Its `.type`
;                            companion names a proven `T` or `T<A>`; its
;                            `.untyped` companion states that nothing else
;                            does — no annotation, a union, a qualified name —
;                            and refuses the name. Of the two, `.type` wins.
;
;   Stated ceilings: a field the class inherits is not its own; a call on a
;   class name (`Fmt.format()`), a chained or a computed receiver keeps its row.
(call_expression
  function: (member_expression object: (identifier) @ref.receiver.variable))

(function_declaration
  parameters: (formal_parameters
    [(required_parameter pattern: (_) @ref.receiver.binding)
     (optional_parameter pattern: (_) @ref.receiver.binding)])) @ref.receiver.binding.scope
(function_expression
  parameters: (formal_parameters
    [(required_parameter pattern: (_) @ref.receiver.binding)
     (optional_parameter pattern: (_) @ref.receiver.binding)])) @ref.receiver.binding.scope
(generator_function_declaration
  parameters: (formal_parameters
    [(required_parameter pattern: (_) @ref.receiver.binding)
     (optional_parameter pattern: (_) @ref.receiver.binding)])) @ref.receiver.binding.scope
(generator_function
  parameters: (formal_parameters
    [(required_parameter pattern: (_) @ref.receiver.binding)
     (optional_parameter pattern: (_) @ref.receiver.binding)])) @ref.receiver.binding.scope
(arrow_function
  parameters: (formal_parameters
    [(required_parameter pattern: (_) @ref.receiver.binding)
     (optional_parameter pattern: (_) @ref.receiver.binding)])) @ref.receiver.binding.scope
(method_definition
  parameters: (formal_parameters
    [(required_parameter pattern: (_) @ref.receiver.binding)
     (optional_parameter pattern: (_) @ref.receiver.binding)])) @ref.receiver.binding.scope
(arrow_function parameter: (identifier) @ref.receiver.binding) @ref.receiver.binding.scope
(catch_clause parameter: (_) @ref.receiver.binding) @ref.receiver.binding.scope
(for_in_statement left: (_) @ref.receiver.binding) @ref.receiver.binding.scope
(lexical_declaration
  (variable_declarator name: (_) @ref.receiver.binding)) @ref.receiver.binding.after
(variable_declaration
  (variable_declarator name: (_) @ref.receiver.binding))

(required_parameter
  pattern: (identifier) @ref.receiver.proof
  type: (type_annotation [(type_identifier) (generic_type name: (type_identifier))] @ref.receiver.proof.type))
(optional_parameter
  pattern: (identifier) @ref.receiver.proof
  type: (type_annotation [(type_identifier) (generic_type name: (type_identifier))] @ref.receiver.proof.type))
(variable_declarator
  name: (identifier) @ref.receiver.proof
  type: (type_annotation [(type_identifier) (generic_type name: (type_identifier))] @ref.receiver.proof.type))
(variable_declarator
  name: (identifier) @ref.receiver.proof
  !type
  value: (new_expression constructor: (identifier) @ref.receiver.proof.literal))

(call_expression
  function: (member_expression
    object: (member_expression
      object: (this)
      property: [(property_identifier) (private_property_identifier)] @ref.receiver.field)))

(public_field_definition
  name: [(property_identifier) (private_property_identifier)] @ref.receiver.declared
  type: (type_annotation [(type_identifier) (generic_type name: (type_identifier))] @ref.receiver.declared.type))
(public_field_definition
  name: [(property_identifier) (private_property_identifier)] @ref.receiver.declared) @ref.receiver.declared.untyped
(required_parameter
  [(accessibility_modifier) (override_modifier) "readonly"] @ref.receiver.declared.field
  pattern: (identifier) @ref.receiver.declared
  type: (type_annotation [(type_identifier) (generic_type name: (type_identifier))] @ref.receiver.declared.type))
(required_parameter
  [(accessibility_modifier) (override_modifier) "readonly"] @ref.receiver.declared.field
  pattern: (identifier) @ref.receiver.declared) @ref.receiver.declared.untyped
(optional_parameter
  [(accessibility_modifier) (override_modifier) "readonly"] @ref.receiver.declared.field
  pattern: (identifier) @ref.receiver.declared
  type: (type_annotation [(type_identifier) (generic_type name: (type_identifier))] @ref.receiver.declared.type))
(optional_parameter
  [(accessibility_modifier) (override_modifier) "readonly"] @ref.receiver.declared.field
  pattern: (identifier) @ref.receiver.declared) @ref.receiver.declared.untyped

(import_statement
  source: (string) @ref.import)

(call_expression
  function: (import)
  arguments: (arguments
    .
    (string) @ref.import))

(program
  (export_statement) @ref.export)

((call_expression
  function: (identifier) @_require
  arguments: (arguments
    .
    (string) @ref.import))
  (#eq? @_require "require"))

;   Type relations (S-535, CR-173, FR-EX-17) — each captured node is a TYPE,
;   recorded as a path-form row of its kind. A type named through a named or
;   default import is recorded qualified by that import's module, exactly as a
;   call through one is (S-440), and binds only within what the import binds
;   to; a type the file declares, or names unimported, binds in the file's own
;   scope. Exactly one type of a kind the relation admits, or nothing.
;
;   @ref.extends      — a class's base (`class A extends B`, `B` an identifier
;                       or a member `ns.B`; a call, `extends Mixin(B)`, names no
;                       type) and an interface's supertypes (`interface I
;                       extends J, K`).
;   @ref.implements   — each type a class `implements`: an interface, and —
;                       TypeScript's types being structural — an abstract
;                       class or a type alias too.
;                       Both are read from a class DECLARATION (plain or
;                       `abstract`) only: a class expression (`const M = class
;                       extends B {}`) declares no class node, so its clause
;                       would become a supertype of the variable or function
;                       around it.
;   @ref.instantiates — the class `new T(…)` constructs.
;   @ref.type_use     — the declared type of a field, parameter, return value,
;                       variable or property signature (every `: T`
;                       annotation), and the type arguments of an `extends` and
;                       of a `new`. A captured type's own type arguments and the
;                       operands of a union, intersection or indexed access
;                       (`Dto | null`, `User['id']`) ride with it: the engine
;                       walks them. A type parameter (`T` in `class Box<T>`) and
;                       a primitive (`string`, `void`) record nothing.
(class_declaration
  (class_heritage
    (extends_clause
      value: (_) @ref.extends)))
(class_declaration
  (class_heritage
    (extends_clause
      type_arguments: (type_arguments (_) @ref.type_use))))
(class_declaration
  (class_heritage
    (implements_clause
      (_) @ref.implements)))
(abstract_class_declaration
  (class_heritage
    (extends_clause
      value: (_) @ref.extends)))
(abstract_class_declaration
  (class_heritage
    (extends_clause
      type_arguments: (type_arguments (_) @ref.type_use))))
(abstract_class_declaration
  (class_heritage
    (implements_clause
      (_) @ref.implements)))
(extends_type_clause
  type: (_) @ref.extends)
(new_expression
  constructor: (_) @ref.instantiates)
(new_expression
  type_arguments: (type_arguments (_) @ref.type_use))
(type_annotation
  (_) @ref.type_use)

;   @ref.access — an own-field access (`this.x`): a method reading a property of
;                 its own class (CR-005, FR-EX-08). The `this` receiver anchors
;                 the capture to an own-member access. Resolution binds it to an
;                 `Accesses` edge only on an exactly-one Field candidate, else it
;                 stays unresolved (NFR-RA-05). A `#private` field is read as
;                 `this.#x`, whose property is a `private_property_identifier`
;                 (S-477), so both spellings are captured. `this.f()` is a call,
;                 not an access: `@ref.access.selection` marks the selection, and
;                 a match whose selection an argument list applies records no
;                 access (S-542; the call's own row is the method call's).
(member_expression
  object: (this)
  property: [(property_identifier) (private_property_identifier)] @ref.access) @ref.access.selection

; ── Argument count (S-591, CR-190, FR-EX-32) ─────────────────────────────────
; The `@arity.*` vocabulary the extraction engine reads (`extract::arity`): every
; argument list, whose named children a call row counts, and the forms that
; make a count unknown. Captures record no row of their own, so every ledger
; target is unchanged.
; `...xs` spreads; a tagged template is not counted; `new T` without
; parentheses passes none.
(arguments) @arity.arguments
(arguments (spread_element) @arity.spread)
(call_expression
  arguments: (template_string) @arity.opaque)
(new_expression
  constructor: (_)
  !arguments) @arity.none
