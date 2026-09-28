; Java reference-extraction query (S-015, capability = "references").
;
;   @ref.call        — a receiver-less invocation's name (`list()`): the scope
;                      hierarchy binds it — the enclosing class, then a static
;                      import naming it (CR-149) — as every other language's
;                      plain call is (Go, Python, PHP, Ruby, C). Recorded as
;                      `Enclosing::list` instead where the enclosing class is
;                      the only scope that could supply it (S-467).
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

;   Receiver shapes (S-467, CR-150 §3.2 A) — MARKERS, recording nothing on
;   their own. Each names the receiver of the `method_invocation` whose
;   `@ref.method` / `@ref.call` row above `collect_refs` may retype to a
;   type-qualified PATH-form `T::name` (`extract::receiver`), when the file
;   proves `T`. The captured node's parent is the invocation — for
;   `@ref.receiver.field`, its grandparent.
;
;   @ref.receiver.name     — a simple name (`mailer.send()`, `Clock.now()`): a
;                            variable the file declares with one type, else a
;                            type its single-type import or own declaration
;                            names.
;   @ref.receiver.field    — `this.x.send()`: the field `x`, from field
;                            positions only (S-398).
;   @ref.receiver.this     — `this.send()`: the enclosing class.
;   @ref.receiver.super    — `super.send()`: the enclosing class's `extends`.
;   @ref.receiver.implicit — `send()`: the enclosing class, where nothing else
;                            in scope could supply the name.
;   @ref.receiver.unproven — a name declared where `DeclaredTypes` does not
;                            read its type: an untyped lambda parameter
;                            (`x -> x.send()`), a for-each variable, a catch
;                            parameter, a pattern variable, a varargs
;                            parameter. The type the file declares for that
;                            name elsewhere is not proven to be this one's.
;
; Anything else — a chained call, `a.b.send()`, `Outer.this.send()` — carries
; no marker and keeps its bare row.

(method_invocation object: (identifier) @ref.receiver.name)
(method_invocation object: (this) @ref.receiver.this)
(method_invocation object: (super) @ref.receiver.super)
(method_invocation
  object: (field_access object: (this) field: (identifier) @ref.receiver.field))
(method_invocation !object name: (identifier) @ref.receiver.implicit)
(lambda_expression parameters: (identifier) @ref.receiver.unproven)
(lambda_expression
  parameters: (inferred_parameters (identifier) @ref.receiver.unproven))
(enhanced_for_statement name: (identifier) @ref.receiver.unproven)
(catch_formal_parameter name: (identifier) @ref.receiver.unproven)
(instanceof_expression name: (identifier) @ref.receiver.unproven)
(type_pattern (identifier) @ref.receiver.unproven)
(record_pattern_component (identifier) @ref.receiver.unproven)
(spread_parameter (variable_declarator name: (identifier) @ref.receiver.unproven))

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
