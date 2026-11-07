; Java reference-extraction query (S-015, capability = "references").
;
;   @ref.call        — a receiver-less invocation's name (`list()`): the scope
;                      hierarchy binds it — the enclosing class, then a static
;                      import naming it (CR-149) — as every other language's
;                      plain call is (Go, Python, PHP, Ruby, C). Recorded as
;                      `Enclosing::list` instead where the enclosing class
;                      declares `list` itself, or no other scope (an outer
;                      class, a static import) could supply it (S-467).
;   @ref.method      — a receiver invocation's name (`service.list()`,
;                      `List.of()`); a bare Method-form row unless its
;                      receiver's type is proven (the `@ref.receiver.*` markers
;                      below, S-467 / CR-150). A receiver call
;                      never takes its target from the file's imports, so the
;                      two shapes must stay apart: recorded alike, `List.of()`
;                      bound to a statically imported in-house `of`.
;   @ref.import      — an import declaration's scoped path
;                      (`org.springframework.web…`, `static a.b.C.m`);
;                      canonicalised (dots → `::`) into the ledger form feeding
;                      the binder and the framework candidacy gate (FR-FW-04).
;   @ref.import.static — the `static` keyword, present in the same match for a
;                      static import; with the wildcard marker it makes the glob
;                      a static one (every static member of the type, not just
;                      its member types).
;   @ref.import.asterisk — present in the same match when the declaration ends
;                      in `.*` (CR-149): the path before it (`a.b.*` → `a::b`,
;                      `static a.b.C.*` → `a::b::C`) is then recorded as a glob —
;                      it names a package or a type whose members it brings into
;                      scope, never one declaration.
;
; One pattern, with the wildcard as an optional marker rather than a second
; pattern kept apart by a last-child anchor: a comment is a named node too, so
; `import a.b.C /* why */;` would slip past such an anchor and record nothing.
;
; Droppable on disk at `.logos/plugins/java/queries/references.scm`.

(method_invocation
  !object
  name: (identifier) @ref.call)

(method_invocation
  object: (_)
  name: (identifier) @ref.method)

;   Receiver markers (S-467, CR-150 §3.2 A; S-514, CR-169) — MARKERS,
;   recording nothing on their own. Each names the receiver of the
;   `method_invocation` whose `@ref.method` / `@ref.call` row above
;   `collect_refs` may retype to a type-qualified PATH-form `T::name`
;   (`extract::receiver`), when the file proves `T`; a `@ref.method` row left
;   bare records its receiver's SHAPE instead — `this` is `self`, `super` is
;   `super`, every other receiver `other` — which the binder dispatches on
;   (FR-EX-13, FR-RS-12). The captured node's parent is the invocation — for
;   `@ref.receiver.field`, its grandparent.
;
;   @ref.receiver.variable — a simple name (`mailer.send()`, `Clock.now()`):
;                            typed by the one binding of it in scope at the
;                            call (below, S-825, CR-256) — else, where no
;                            binding anywhere in the file declares the name
;                            and no static import brings it in, a type its
;                            single-type import or own declaration names, or
;                            `type_name` (next) marks.
;   @ref.receiver.type_name — a simple name spelt as Java names a type
;                            (S-854): capitalised, and not a constant's
;                            all-capitals name (`Helper`, `T`, never `LOG`).
;                            Where no binding or static import of the file
;                            declares it, `Helper.slug()` is the row
;                            `Helper::slug`, which the package rungs scope: a
;                            type of the caller's own package binds without
;                            an import.
;   @ref.receiver.field    — `this.x.send()`: the field `x`, from field
;                            positions only (S-398).
;   @ref.receiver.this     — `this.send()`: the enclosing class.
;   @ref.receiver.super    — `super.send()`: the enclosing class's `extends`.
;   @ref.receiver.implicit — `send()`: the enclosing class, where it declares
;                            the name itself or nothing else in scope (an
;                            outer class, a static import) could supply it.
;   @ref.receiver.refused  — `Outer.super.send()` (the `super` after a
;                            qualifying name, not a field of it): its receiver
;                            is `Outer`'s superclass, which the `name` marker on
;                            the same invocation would read as `Outer` itself.
;                            Overrides every other marker.
;   @ref.receiver.other    — every receiver (S-514): the shape of a call the
;                            markers above prove nothing for — a chained call,
;                            `a.b.send()`, `Outer.this.send()` — is `other`.
;                            `this` and `super` above outrank it.
;   @ref.receiver.anonymous — an anonymous class body (`new T() { … }`): typing
;                            stops there, and a `this` / `super` call inside one
;                            is `other`, its instance having no class node —
;                            except a `this.m()` to an `m` the body declares,
;                            recorded as the free call the lexical scope binds.

(method_invocation object: (identifier) @ref.receiver.variable)
((method_invocation object: (identifier) @ref.receiver.type_name)
  (#match? @ref.receiver.type_name "^[A-Z]([A-Za-z0-9_$]*[a-z][A-Za-z0-9_$]*)?$"))
(method_invocation object: (this) @ref.receiver.this)
(method_invocation object: (super) @ref.receiver.super)
(method_invocation
  object: (field_access object: (this) field: (identifier) @ref.receiver.field))
(method_invocation !object name: (identifier) @ref.receiver.implicit)
(method_invocation object: (_) (super) @ref.receiver.refused)
(method_invocation object: (_) @ref.receiver.other)
(object_creation_expression (class_body) @ref.receiver.anonymous)

; Plain-name receiver typing (S-825, CR-256, FR-RS-10) — the vocabulary Rust,
; TypeScript, Python, PHP, Kotlin, C++ and C# speak (`extract::receiver`),
; recording no row of its own. `x.send()` is typed `T::send` when exactly one
; binding of `x` is in scope at the call and it carries a proof; the row is the
; Path `T::send` the package rung binds, as S-467's was:
;
;   @ref.receiver.binding  — each name a declaration binds, and where:
;                            `.scope` the method, constructor, lambda or record
;                            a parameter belongs to (a record component is in
;                            scope in its whole record), the enhanced `for` or
;                            `catch` statement whose variable it is, an
;                            anonymous class or enum constant body its field
;                            is declared in;
;                            `.through` a local's block, constructor body,
;                            `switch` block (a case group's local) or `for`
;                            statement, and a resource's `try` block, from
;                            where the name is written;
;                            `.enclosing` a named class's, enum's or record's
;                            field, in that class's members and in the classes
;                            nested in it reached through classes that record
;                            no supertype — a parameter or local declared
;                            inside the class hides it, it hides one declared
;                            outside the class (a local class's field, not the
;                            enclosing method's local), and a nearer class's
;                            field of its name shadows it;
;                            `.scope` an interface constant's whole
;                            interface body, its nested classes included, and
;                            an enum constant's whole enum body.
;   @ref.receiver.proof    — the binding's type: `.type` a declared `T` or
;                            `T<A>` (read `T`). A binding with no proof — an
;                            untyped lambda parameter, an enhanced-`for`,
;                            `catch`, pattern or varargs variable, an interface
;                            or enum constant, a `var`, an array, a qualified
;                            (`com.b.Mailer`, `Map.Entry`) or annotated type —
;                            proves nothing, and hides every binding of its
;                            name it is nearer than.
;   @ref.receiver.flows    — a pattern variable (`o instanceof T t`, a type or
;                            record pattern): in scope from where it is written
;                            to the end of the innermost
;                            @ref.receiver.flow_scope around it (a block, a
;                            constructor body, a switch rule or case group, a
;                            callable, a field or constant declaration). It has
;                            no proof, so a scope wider than the language's
;                            only refuses more.
;   @ref.receiver.hides    — every name a declaration declares. One no binding
;                            above states is bound with no proof across the
;                            innermost @ref.receiver.hiding_scope (a callable,
;                            an initializer), so an outer binding is never the
;                            one in scope there.
;
; Stated ceilings: a field the class inherits from another file is not in
; scope; neither is an outer class's field in a class nested through one that
; records a supertype, nor any named class's field inside an anonymous class
; body.

; Parameters: of a method, constructor, lambda or record.
(_
  parameters: (formal_parameters (formal_parameter name: (identifier) @ref.receiver.binding))) @ref.receiver.binding.scope
(_
  parameters: (formal_parameters
    (spread_parameter (variable_declarator name: (identifier) @ref.receiver.binding)))) @ref.receiver.binding.scope
(lambda_expression parameters: (identifier) @ref.receiver.binding) @ref.receiver.binding.scope
(lambda_expression
  parameters: (inferred_parameters (identifier) @ref.receiver.binding)) @ref.receiver.binding.scope

; Fields: of a named class, record or enum, by the nearest declaration; an
; interface's constants, in its whole body, so a class nested in it never
; reads an outer class's field of the name; an anonymous class body's or an
; enum constant body's fields, in that body.
(class_declaration
  body: (class_body
    (field_declaration
      declarator: (variable_declarator name: (identifier) @ref.receiver.binding)))) @ref.receiver.binding.enclosing
(record_declaration
  body: (class_body
    (field_declaration
      declarator: (variable_declarator name: (identifier) @ref.receiver.binding)))) @ref.receiver.binding.enclosing
(enum_declaration
  body: (enum_body
    (enum_body_declarations
      (field_declaration
        declarator: (variable_declarator name: (identifier) @ref.receiver.binding))))) @ref.receiver.binding.enclosing
(interface_declaration
  body: (interface_body
    (constant_declaration
      declarator: (variable_declarator name: (identifier) @ref.receiver.binding))) @ref.receiver.binding.scope)
(object_creation_expression
  (class_body
    (field_declaration
      declarator: (variable_declarator name: (identifier) @ref.receiver.binding))) @ref.receiver.binding.scope)
(enum_constant
  body: (class_body
    (field_declaration
      declarator: (variable_declarator name: (identifier) @ref.receiver.binding))) @ref.receiver.binding.scope)

; Enum constants: variables of their enum, in scope in its whole body. One
; obscures a type of its name (`Mailer.send()` beside a constant `Mailer`)
; and proves nothing.
(enum_declaration
  body: (enum_body (enum_constant name: (identifier) @ref.receiver.binding)) @ref.receiver.binding.scope)

; Locals, from where each is written — its own and every later declarator's
; initializer — to the end of its block, constructor body or `for` statement,
; and a case group's local to the end of its whole `switch` block, every later
; group of which it is in scope in. Never before it: the field of its name is
; in scope there.
(block
  (local_variable_declaration
    declarator: (variable_declarator name: (identifier) @ref.receiver.binding))) @ref.receiver.binding.through
(constructor_body
  (local_variable_declaration
    declarator: (variable_declarator name: (identifier) @ref.receiver.binding))) @ref.receiver.binding.through
(switch_block
  (switch_block_statement_group
    (local_variable_declaration
      declarator: (variable_declarator name: (identifier) @ref.receiver.binding)))) @ref.receiver.binding.through
(for_statement
  init: (local_variable_declaration
    declarator: (variable_declarator name: (identifier) @ref.receiver.binding))) @ref.receiver.binding.through

; Enhanced `for` and `catch` variables: their statement. A resource: from
; where it is written to the end of its `try` block — never in the `catch` or
; `finally` clauses, where the field of its name is in scope.
(enhanced_for_statement name: (identifier) @ref.receiver.binding) @ref.receiver.binding.scope
(catch_clause (catch_formal_parameter name: (identifier) @ref.receiver.binding)) @ref.receiver.binding.scope
(try_with_resources_statement
  resources: (resource_specification (resource name: (identifier) @ref.receiver.binding))
  body: (block) @ref.receiver.binding.through)

; Pattern variables: from where each is written to the end of the innermost
; flow scope around it. Java scopes them by flow (`if (!(o instanceof T t))
; return; t.m();`), never beyond the block holding them.
(instanceof_expression name: (identifier) @ref.receiver.flows)
(type_pattern (identifier) @ref.receiver.flows)
(record_pattern_component (identifier) @ref.receiver.flows)
(block) @ref.receiver.flow_scope
(constructor_body) @ref.receiver.flow_scope
(switch_rule) @ref.receiver.flow_scope
(switch_block_statement_group) @ref.receiver.flow_scope
(_ parameters: (_)) @ref.receiver.flow_scope
(field_declaration) @ref.receiver.flow_scope
(constant_declaration) @ref.receiver.flow_scope

; A declared simple type `T` or `T<A>`, wherever the declaration sits — never
; `var`, an array (`T[] x`, `T x[]`), a qualified or an annotated type.
(field_declaration
  type: [(type_identifier) (generic_type (type_identifier))] @ref.receiver.proof.type
  declarator: (variable_declarator name: (identifier) @ref.receiver.proof !dimensions))
(local_variable_declaration
  type: [(type_identifier) (generic_type (type_identifier))] @ref.receiver.proof.type
  declarator: (variable_declarator name: (identifier) @ref.receiver.proof !dimensions)
  (#not-eq? @ref.receiver.proof.type "var"))
(formal_parameter
  type: [(type_identifier) (generic_type (type_identifier))] @ref.receiver.proof.type
  name: (identifier) @ref.receiver.proof
  !dimensions
  (#not-eq? @ref.receiver.proof.type "var"))
(resource
  type: [(type_identifier) (generic_type (type_identifier))] @ref.receiver.proof.type
  name: (identifier) @ref.receiver.proof
  !dimensions
  (#not-eq? @ref.receiver.proof.type "var"))

; Every name a declaration declares, and the scopes an unstated one hides its
; name across.
(variable_declarator name: (identifier) @ref.receiver.hides)
(formal_parameter name: (identifier) @ref.receiver.hides)
(catch_formal_parameter name: (identifier) @ref.receiver.hides)
(resource name: (identifier) @ref.receiver.hides)
(enhanced_for_statement name: (identifier) @ref.receiver.hides)
(inferred_parameters (identifier) @ref.receiver.hides)
(lambda_expression parameters: (identifier) @ref.receiver.hides)
(_ parameters: (_)) @ref.receiver.hiding_scope
(static_initializer) @ref.receiver.hiding_scope
(class_body (block) @ref.receiver.hiding_scope)

(import_declaration
  "static"? @ref.import.static
  (scoped_identifier) @ref.import
  (asterisk)? @ref.import.asterisk)

;   @ref.access — an own-field access (`this.x`): a method reading a field of
;                 its own class (CR-005, FR-EX-08). `field_access` is a distinct
;                 node from `method_invocation`, so this never double-captures a
;                 method call. The class lexically Contains both the method and
;                 its `field` declarations (symbols.scm captures Java fields), so
;                 resolution binds an exactly-one Field candidate to an
;                 `Accesses` edge (Method → Field); an ambiguous access stays
;                 unresolved (NFR-RA-05). This is the bound LCOM4 input.
(field_access
  object: (this)
  field: (identifier) @ref.access)
;   Type relations (S-466, CR-149 §3.2 B, FR-EX-10). Each capture is a TYPE
;   node; `collect_refs` turns it into PATH-form rows — the head type (generics
;   stripped, `a.b.C` → `a::b::C`) under the capture's edge kind, and every type
;   argument inside it (`List<Dto>` → `Dto`) as a `TypeUses` of the same
;   declaration. Never method-form: a Method-form `::` target is the binder's
;   trait-dispatch branch (S-281). A name the enclosing declarations declare as a
;   type parameter (`T` in `class Box<T>`) is a type variable, not a type, and
;   records nothing.
;
;   @ref.extends     — a class's superclass, an interface's super-interfaces:
;                      `Extends` (class → class, interface → interface).
;   @ref.implements  — a class's, enum's or record's super-interfaces:
;                      `Implements` (→ interface).
;   @ref.instantiates — the type of `new T(…)`: `Instantiates` (→ class). It
;                      binds to the class — constructors are not nodes
;                      (symbols.scm); an anonymous `new I() {…}` of an interface
;                      names no class and stays unresolved.
;   @ref.type_use    — a declared type: a field's (attributed to the field), an
;                      interface constant's, a method's return type, a
;                      parameter's (record components included), a local's, a
;                      for-each variable's, a resource's, a caught exception's:
;                      `TypeUses` (declaration → type).
;
; Deliberately NOT captured: annotations (`@interface` types are not extracted
; as nodes), casts, `instanceof`, class literals, `throws`, type bounds and
; method references — not declarations of a type.

(class_declaration
  superclass: (superclass (_) @ref.extends))

(interface_declaration
  (extends_interfaces (type_list (_) @ref.extends)))

(class_declaration
  interfaces: (super_interfaces (type_list (_) @ref.implements)))

(enum_declaration
  interfaces: (super_interfaces (type_list (_) @ref.implements)))

(record_declaration
  interfaces: (super_interfaces (type_list (_) @ref.implements)))

(object_creation_expression
  type: (_) @ref.instantiates)

(field_declaration type: (_) @ref.type_use)
(constant_declaration type: (_) @ref.type_use)
(method_declaration type: (_) @ref.type_use)
(formal_parameter type: (_) @ref.type_use)
(spread_parameter (_) @ref.type_use)
(local_variable_declaration type: (_) @ref.type_use)
(enhanced_for_statement type: (_) @ref.type_use)
(resource type: (_) @ref.type_use)
(catch_formal_parameter (catch_type (_) @ref.type_use))

; ── Argument count (S-591, CR-190, FR-EX-32) ─────────────────────────────────
; The `@arity.*` vocabulary the extraction engine reads (`extract::arity`): every
; argument list, whose named children a call row counts, and the forms that
; make a count unknown. Captures record no row of their own, so every ledger
; target is unchanged.
; A `method_invocation`'s `object` is its receiver, not its callee.
(argument_list) @arity.arguments
(method_invocation object: (_) @arity.receiver)
