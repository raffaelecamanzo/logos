; Scala reference-extraction query (S-061, CR-009, capability = "references").
;
;   @ref.method — a method/function invocation's name, by simple name
;                 (`compute()`) or as the selected member of a receiver that is
;                 not an object (`helper.doThing()`, `xs.size`, `a max b`). A
;                 Method-form row that binds by its
;                 receiver's SHAPE, recorded by the `@ref.receiver.*` markers
;                 below (S-514, S-516, CR-169), yielding a `Calls` edge — or,
;                 where the file proves its receiver's type `T`, the Path row
;                 `T::doThing` (S-576, "Receiver typing" below).
;   @ref.call   — a call on an object (S-573, FR-RS-36): `StringUtil.decode(t)`
;                 and the parameterless `StringUtil.size`, recorded with the
;                 object (`@ref.call.scope`) as the Path `StringUtil::decode` and
;                 bound among that object's members through the file's imports,
;                 its declared package and its wildcards — never through the
;                 caller's own class. A receiver is read as an object when it is
;                 one capitalised name; a value of that spelling the file
;                 declares (`@ref.call.shadow`) drops the row instead.
;   @ref.selection — a call written with no argument list of its own (S-573): a
;                 parameterless member selection `a.size`, which binds only a
;                 method of the name (never a `val`), and an infix call `a op b`.
;                 Its row records no argument count, and a selection an argument
;                 list applies (`a.m(x)`, `a.m[T](x)`) or that an assignment
;                 writes (`a.x = v`) is not one. A symbolic operator (`a + b`)
;                 records nothing.
;   @ref.instantiates — the class of `new T(…)`, `new a.b.T[A](…)` and `new T
;                 { … }` (S-573): `Instantiates`, the head type alone — a type
;                 argument names no type here, as a Scala type parameter is
;                 never told apart from a class.
;   @ref.call.shadow — a capitalised value the file declares (a `val`, `var`
;                 or parameter): a `Codec.length()` on it is a call on that
;                 value, so its object-qualified row is dropped (S-553's rule).
;   @ref.access — an own-field access (`this.x`): a method reading a field of
;                 its own class (CR-005, FR-EX-08), the bound LCOM4 input. The
;                 class lexically Contains both the method and its fields — its
;                 `val`/`var` members and its constructor parameters (S-575) —
;                 so resolution binds an exactly-one Field candidate to an
;                 `Accesses` edge; an ambiguous/unmatched access stays
;                 unresolved (NFR-RA-05).
;   @ref.access.bare — a bare name in a value position (`semaphore`, `rate`,
;                 the `label` of `s"$label"`): an `Accesses` row of the method
;                 when the enclosing class declares a field of that name and no
;                 local or parameter of the method does (S-575, FR-EX-28). See
;                 "Bare field access" below.
;   @ref.access.shadow — a name a method declares (a parameter, a local, a
;                 pattern, lambda or generator variable): it records no row; it
;                 drops the `@ref.access.bare` rows of that method headed by its
;                 name.
;   @ref.extends — each entry of a class's, object's, trait's or enum's
;                 `extends A with B with C` clause (S-575, FR-RS-15), the head
;                 type only. The clause does not say which entry is the class —
;                 a trait may head it — so the plugin declares
;                 `supertype_kind_follows_target`: an entry binds the one
;                 in-repository class or trait it names, through the imports,
;                 the declared package and the wildcards, and its edge is
;                 `Extends` to a class, `Implements` to a trait. A self-type
;                 (`self: A with B =>`) is no supertype and records nothing.
;
; Receiver-shape markers (S-516, FR-EX-13, FR-RS-12) — they record no row of
; their own; each names the receiver of the `@ref.method` that shares its parent
; node (the `field_expression`, or for a bare call the `call_expression`).
; `this` and `super` parse as plain identifiers, so they are told apart by text:
;
;   @ref.receiver.self      — `this.compute()`: binds among the enclosing
;                             class's, object's or trait's own members only.
;                             A qualified `Outer.this` names another instance,
;                             so it is `other`.
;   @ref.receiver.super     — `super.compute()`: binds only through a proven
;                             `Extends` (S-575): the nearest base class that
;                             declares exactly one `compute`.
;   @ref.receiver.other     — every other receiver (`helper.doThing()`): never
;                             bound through the caller's scope
;                             (`no-receiver-evidence`).
;   @ref.receiver.implicit  — `compute()`, no receiver: under this plugin's
;                             `implicit_receiver = "self"` a call on the current
;                             instance inside a class, object or trait, a free
;                             call outside one (a Scala 3 top-level `def`).
;   @ref.receiver.anonymous — a `new T { … }` body: its instance has no node,
;                             so a `this.` call inside records `other` and a
;                             bare call is a free call.
;
; Droppable on disk at `.logos/plugins/scala/queries/references.scm`.
;
;   @ref.import.dotted — an `import` declaration (S-518, CR-170, FR-RS-13):
;                 `import a.b.C`, `import a.b._` / `a.b.*`, `import a.b.{C, D}`
;                 and comma-separated paths. Scala flattens a dotted path into
;                 repeated `path:` identifiers with no single spanning node, so
;                 the whole declaration is captured and walked structurally
;                 (`extract::refs::flatten_dotted_import`, the twin of Rust's
;                 `ref.use` walk): one row per imported path, a wildcard a
;                 `Glob` row aliased `*` — it brings every member of the package
;                 or object into view, a top-level `def` and an object's method
;                 included (S-573). The binder resolves each through the
;                 declared package's fully-qualified index.

; A call by simple name: `compute()`, `assert(...)`, the `test`/`it` markers,
; and a generic one, `fork[T](f)`.
; An auxiliary constructor's `def this(a: Int) = this(a, 0)` delegates to the
; primary constructor, which has no node: read as a call named `this`, it would
; bind the auxiliary constructor to itself, so it is not captured.
((call_expression
  function: (identifier) @ref.method @ref.receiver.implicit)
  (#not-eq? @ref.method "this"))
(call_expression
  function: (generic_function
    function: (identifier) @ref.method @ref.receiver.implicit))

; A call selecting a member of a receiver: `helper.doThing()`,
; `helper.doThing[T]()` — the method name. A receiver that is one capitalised
; name is an object: its call is the qualified `@ref.call` below.
((call_expression
  function: (field_expression
    value: (_) @_receiver
    field: (identifier) @ref.method))
  (#not-match? @_receiver "^[A-Z][A-Za-z0-9_]*$"))
((call_expression
  function: (generic_function
    function: (field_expression
      value: (_) @_receiver
      field: (identifier) @ref.method)))
  (#not-match? @_receiver "^[A-Z][A-Za-z0-9_]*$"))

; A call on an object (S-573): `StringUtil.decode(t)`, `Chain.empty[A]()`.
((call_expression
  function: (field_expression
    value: (identifier) @ref.call.scope
    field: (identifier) @ref.call))
  (#match? @ref.call.scope "^[A-Z][A-Za-z0-9_]*$"))
((call_expression
  function: (generic_function
    function: (field_expression
      value: (identifier) @ref.call.scope
      field: (identifier) @ref.call)))
  (#match? @ref.call.scope "^[A-Z][A-Za-z0-9_]*$"))

; A parameterless member selection (S-573): `StringUtil.size` on an object,
; `xs.size` on any other receiver — a lower-case member, since a capitalised one
; is a nested object or type. `this.x` is the own-field access below, and
; `super.x` names an inherited member no shape binds; `Outer.this` names an
; instance, no member. Every member selection is
; matched here, an applied one too; the extractor keeps only those no argument
; list applies (`@ref.selection`).
((field_expression
  value: (identifier) @ref.call.scope
  field: (identifier) @ref.call) @ref.selection
  (#match? @ref.call.scope "^[A-Z][A-Za-z0-9_]*$")
  (#match? @ref.call "^[a-z_]")
  (#not-any-of? @ref.call "this" "super"))
((field_expression
  value: (_) @_receiver
  field: (identifier) @ref.method) @ref.selection
  (#not-match? @_receiver "^(this|super|[A-Z][A-Za-z0-9_]*)$")
  (#match? @ref.method "^[a-z_]")
  (#not-any-of? @ref.method "this" "super"))

; An infix call `a max b` (S-573) is `a.max(b)`: the operator is the method,
; the left operand its receiver. Only an alphanumeric operator: a symbolic one
; (`+`, `::`, `==`) is the arithmetic and collection vocabulary of the standard
; library.
((infix_expression
  operator: (identifier) @ref.method) @ref.selection)
((infix_expression
  left: (identifier) @ref.receiver.self)
  (#eq? @ref.receiver.self "this"))
(infix_expression
  left: (_) @ref.receiver.other)

; `new T(…)` instantiates `T` (S-573): the head type only.
(instance_expression
  [(type_identifier) (stable_type_identifier)] @ref.instantiates)
(instance_expression
  (generic_type
    type: [(type_identifier) (stable_type_identifier)] @ref.instantiates))

; Each supertype of a class, object or trait (S-575, FR-EX-28): every entry of
; `extends A(x) with B with C[T]`, the head type only, as `new T` records it.
; The clause's entries share one field name, so they are matched without it
; (a field pattern would match the first entry alone). An enum case's own
; clause (`case Red extends Color(1)`) names its enum, and is not read: a case
; is no declaration, so its row would be the enum's and name the enum itself.
(enum_definition
  (extends_clause [(type_identifier) (stable_type_identifier)] @ref.extends))
(enum_definition
  (extends_clause
    (generic_type type: [(type_identifier) (stable_type_identifier)] @ref.extends)))
(class_definition
  (extends_clause [(type_identifier) (stable_type_identifier)] @ref.extends))
(class_definition
  (extends_clause
    (generic_type type: [(type_identifier) (stable_type_identifier)] @ref.extends)))
(object_definition
  (extends_clause [(type_identifier) (stable_type_identifier)] @ref.extends))
(object_definition
  (extends_clause
    (generic_type type: [(type_identifier) (stable_type_identifier)] @ref.extends)))
(trait_definition
  (extends_clause [(type_identifier) (stable_type_identifier)] @ref.extends))
(trait_definition
  (extends_clause
    (generic_type type: [(type_identifier) (stable_type_identifier)] @ref.extends)))

; ── Free calls (S-573, FR-RS-36; `extract::free_call`) ─────────────────────────
; A bare call inside a class, object or trait is read among that type's members
; first (`@ref.receiver.implicit`, `self`). Where the file shows that no member
; answers the name, it is the free call the imports and the package bind:
; `forkDiscard(update())` inside `object RateLimiter` calls the top-level
; `ox.forkDiscard`. Three marks, recording no row of their own:
;
;   @ref.implicit.free  — a bare call's name that may be free; a name every
;                         instance has (`AnyRef`'s members) is never marked.
;   @ref.implicit.open  — a body whose members the file cannot all see: one
;                         that inherits (`extends`, a self-type, `new T { … }`),
;                         or one the
;                         compiler adds members to (a case class or object, an
;                         enum). A call inside one stays on the instance.
;   @ref.implicit.value — a name the file binds as a value — a parameter, a
;                         class parameter, a local, a named `given`, a pattern
;                         variable (`case f :: rest`, `val (a, b)`, `x @ p`) or
;                         a lambda variable: a call of it applies that value,
;                         and stays on the instance wherever the file writes it.
((call_expression
  function: (identifier) @ref.implicit.free)
  (#not-any-of? @ref.implicit.free
    "this" "toString" "hashCode" "equals" "getClass" "synchronized" "wait" "notify"
    "notifyAll" "clone" "finalize" "eq" "ne" "isInstanceOf" "asInstanceOf"))
((call_expression
  function: (generic_function
    function: (identifier) @ref.implicit.free))
  (#not-any-of? @ref.implicit.free "isInstanceOf" "asInstanceOf"))

(class_definition
  extend: (extends_clause)
  body: (template_body) @ref.implicit.open)
(object_definition
  extend: (extends_clause)
  body: (template_body) @ref.implicit.open)
(trait_definition
  extend: (extends_clause)
  body: (template_body) @ref.implicit.open)
(class_definition
  "case"
  body: (template_body) @ref.implicit.open)
(object_definition
  "case"
  body: (template_body) @ref.implicit.open)
(enum_definition
  body: (enum_body) @ref.implicit.open)
((template_body
  (self_type)) @ref.implicit.open)
(instance_expression
  (template_body) @ref.implicit.open)

(parameter name: (identifier) @ref.implicit.value)
(class_parameter name: (identifier) @ref.implicit.value)
(val_definition pattern: (identifier) @ref.implicit.value)
(var_definition pattern: (identifier) @ref.implicit.value)
(val_declaration name: (identifier) @ref.implicit.value)
(var_declaration name: (identifier) @ref.implicit.value)
(tuple_pattern (identifier) @ref.implicit.value)
(case_clause pattern: (identifier) @ref.implicit.value)
(case_class_pattern pattern: (identifier) @ref.implicit.value)
(typed_pattern pattern: (identifier) @ref.implicit.value)
(capture_pattern name: (identifier) @ref.implicit.value)
(infix_pattern left: (identifier) @ref.implicit.value)
(infix_pattern right: (identifier) @ref.implicit.value)
(named_pattern (identifier) @ref.implicit.value)
(repeat_pattern pattern: (identifier) @ref.implicit.value)
(given_definition name: (identifier) @ref.implicit.value)
(lambda_expression parameters: (identifier) @ref.implicit.value)
(binding name: (identifier) @ref.implicit.value)
(enumerator . (identifier) @ref.implicit.value)

; A capitalised value the file declares (S-573): never an object.
(val_definition
  pattern: (identifier) @ref.call.shadow (#match? @ref.call.shadow "^[A-Z]"))
(var_definition
  pattern: (identifier) @ref.call.shadow (#match? @ref.call.shadow "^[A-Z]"))
(parameter
  name: (identifier) @ref.call.shadow (#match? @ref.call.shadow "^[A-Z]"))
(class_parameter
  name: (identifier) @ref.call.shadow (#match? @ref.call.shadow "^[A-Z]"))

; The receiver of a member call.
((field_expression
  value: (identifier) @ref.receiver.self)
  (#eq? @ref.receiver.self "this"))

((field_expression
  value: (identifier) @ref.receiver.super)
  (#eq? @ref.receiver.super "super"))

(field_expression
  value: (_) @ref.receiver.other)

(instance_expression
  (template_body) @ref.receiver.anonymous)

; Receiver typing (S-576, CR-183, FR-RS-37, FR-RS-10) — Java's typing markers
; (`extract::receiver`), recording no row of their own. Where the file proves a
; member call's receiver type `T`, its Method row is retyped to the Path-form
; `T::ship` and keeps its `other` shape: it is a call on an instance of `T`,
; not the written `T.ship()`, so it binds among `T`'s own members and up its
; proven supertypes — and, where `T` has a companion `object`, among the
; class's methods, never the object's (the plugin's `companion_objects`);
; anything unproven keeps its `other` row (`no-receiver-evidence`):
;
;   @ref.receiver.name     — `order.ship()`: the one type the file declares
;                            the name with, where it is declared in scope at
;                            the call — a class parameter, a member `val`/`var`
;                            the enclosing class, object or trait defines (an
;                            abstract `val` is no field), or a parameter or
;                            local of the method. A capitalised
;                            receiver is an object's Path row above, never this
;                            marker.
;   @ref.receiver.field    — `this.order.ship()`: the class parameter or member
;                            `val`/`var` `order` the enclosing class declares
;                            itself.
;   @ref.receiver.declared — a `val`/`var` (`pattern:`, which the declaration
;                            walk does not read as a name): with a `.type`
;                            companion its declared type, or the class of a
;                            `new T(…)` it is initialised with; with
;                            `.untyped`, none (`val got = make()`), which
;                            refuses the name.
;   @ref.receiver.unproven — a name bound where no type is read — a lambda or
;                            pattern variable, a generator's, a `val (a, b)` or
;                            `val a, b`, a `given`, a `new T { … }` value — or
;                            declared with a type whose simple name is not the
;                            type (`a.b.Order`, whose simple name the imports
;                            may re-qualify to another type; `=> a.b.Order`,
;                            `A with b.C`, `x.type`). Refused in the whole file.
;
; A type parameter (`class Box[T](t: T)`, `def f[A](a: A)`) is no type, and an
; inherited member is not in scope: `t.ship()` and a call on a member a base
; class declares keep their bare row.
;
; Only a call an argument list applies is typed. A parameterless selection
; (`config.schedule`) reads a `val` and a parameterless `def` alike, and a
; selection binds only a method (S-573): on a case class whose field
; `schedule` sits beside its copy-builder `def schedule(s: Schedule)`, the
; ox idiom, the field read would bind the builder. It keeps its `other` row.
; Stated ceilings:
; - on a class with a companion `object`, a typed row binds among the pair's
;   methods that take an instance receiver alone, and climbs no supertype: a
;   method the class inherits is a miss there;
; - a name is in scope at a call where the outermost callable around it
;   declares it anywhere — a nested `def`'s or a lambda's typed parameter
;   included — and `this.x` reads the file's field declarations, a local
;   class's parameters not among them (the mechanism's rules, shared with Java
;   and C#).
((call_expression
  function: (field_expression
    value: (identifier) @ref.receiver.name))
  (#not-match? @ref.receiver.name "^(this|super|[A-Z][A-Za-z0-9_]*)$"))
((call_expression
  function: (generic_function
    function: (field_expression
      value: (identifier) @ref.receiver.name)))
  (#not-match? @ref.receiver.name "^(this|super|[A-Z][A-Za-z0-9_]*)$"))

((call_expression
  function: (field_expression
    value: (field_expression
      value: (identifier) @_this
      field: (identifier) @ref.receiver.field)))
  (#eq? @_this "this"))
((call_expression
  function: (generic_function
    function: (field_expression
      value: (field_expression
        value: (identifier) @_this
        field: (identifier) @ref.receiver.field))))
  (#eq? @_this "this"))

(val_definition
  pattern: (identifier) @ref.receiver.declared
  type: (_) @ref.receiver.declared.type)
(var_definition
  pattern: (identifier) @ref.receiver.declared
  type: (_) @ref.receiver.declared.type)
(val_definition
  !type
  pattern: (identifier) @ref.receiver.declared @ref.receiver.declared.untyped)
(var_definition
  !type
  pattern: (identifier) @ref.receiver.declared @ref.receiver.declared.untyped)
(val_definition
  !type
  pattern: (identifier) @ref.receiver.declared
  value: (instance_expression
    [(type_identifier) @ref.receiver.declared.type
     (generic_type type: (type_identifier) @ref.receiver.declared.type)]))
(var_definition
  !type
  pattern: (identifier) @ref.receiver.declared
  value: (instance_expression
    [(type_identifier) @ref.receiver.declared.type
     (generic_type type: (type_identifier) @ref.receiver.declared.type)]))

(val_definition
  pattern: (identifier) @ref.receiver.unproven
  value: (instance_expression (template_body)))
(var_definition
  pattern: (identifier) @ref.receiver.unproven
  value: (instance_expression (template_body)))
(val_definition pattern: (identifiers (identifier) @ref.receiver.unproven))
(var_definition pattern: (identifiers (identifier) @ref.receiver.unproven))
(lambda_expression parameters: (identifier) @ref.receiver.unproven)
; `implicit request =>`: the `parameters:` field's first node is the keyword,
; and a field pattern matches the first alone.
(lambda_expression "implicit" . (identifier) @ref.receiver.unproven)
(binding !type name: (identifier) @ref.receiver.unproven)
(tuple_pattern (identifier) @ref.receiver.unproven)
(case_clause pattern: (identifier) @ref.receiver.unproven)
; Scala 3's brace-less `catch case e =>`, which is no `case_clause`.
(catch_clause pattern: (identifier) @ref.receiver.unproven)
; Every variable of `R(_, a, b)`: its `pattern:` fields repeat, and a field
; pattern would match the first alone.
(case_class_pattern (identifier) @ref.receiver.unproven)
(typed_pattern pattern: (identifier) @ref.receiver.unproven)
(capture_pattern name: (identifier) @ref.receiver.unproven)
(infix_pattern left: (identifier) @ref.receiver.unproven)
(infix_pattern right: (identifier) @ref.receiver.unproven)
(named_pattern (identifier) @ref.receiver.unproven)
(repeat_pattern pattern: (identifier) @ref.receiver.unproven)
(given_definition name: (identifier) @ref.receiver.unproven)
; A self-type alias names the instance (`class Inner { outer => … }`).
(self_type . (identifier) @ref.receiver.unproven)
(enumerator . (identifier) @ref.receiver.unproven)
; An interpolated-string pattern binds each `$name` it spells (`case s"#$id"`):
; in a pattern position only, since in an expression it is a use.
(case_clause pattern: (interpolated_string_expression
  (interpolated_string (interpolation (identifier) @ref.receiver.unproven))))
(catch_clause pattern: (interpolated_string_expression
  (interpolated_string (interpolation (identifier) @ref.receiver.unproven))))
(case_class_pattern (interpolated_string_expression
  (interpolated_string (interpolation (identifier) @ref.receiver.unproven))))
(infix_pattern left: (interpolated_string_expression
  (interpolated_string (interpolation (identifier) @ref.receiver.unproven))))
(infix_pattern right: (interpolated_string_expression
  (interpolated_string (interpolation (identifier) @ref.receiver.unproven))))
(val_definition pattern: (interpolated_string_expression
  (interpolated_string (interpolation (identifier) @ref.receiver.unproven))))
(enumerator . (interpolated_string_expression
  (interpolated_string (interpolation (identifier) @ref.receiver.unproven))))

; A declared type that is not one simple name, or one generic one: every type
; form but `type_identifier` and `generic_type` over one, on each declaration
; that carries a `type:`.
(parameter
  name: (identifier) @ref.receiver.unproven
  type: [(stable_type_identifier) (generic_type type: (stable_type_identifier)) (singleton_type)
         (projected_type) (function_type) (compound_type) (infix_type) (lazy_parameter_type)
         (repeated_parameter_type) (annotated_type) (tuple_type) (named_tuple_type)
         (structural_type) (literal_type) (match_type) (type_lambda) (applied_constructor_type)
         (wildcard)])
(class_parameter
  name: (identifier) @ref.receiver.unproven
  type: [(stable_type_identifier) (generic_type type: (stable_type_identifier)) (singleton_type)
         (projected_type) (function_type) (compound_type) (infix_type) (lazy_parameter_type)
         (repeated_parameter_type) (annotated_type) (tuple_type) (named_tuple_type)
         (structural_type) (literal_type) (match_type) (type_lambda) (applied_constructor_type)
         (wildcard)])
(binding
  name: (identifier) @ref.receiver.unproven
  type: [(stable_type_identifier) (generic_type type: (stable_type_identifier)) (singleton_type)
         (projected_type) (function_type) (compound_type) (infix_type) (lazy_parameter_type)
         (repeated_parameter_type) (annotated_type) (tuple_type) (named_tuple_type)
         (structural_type) (literal_type) (match_type) (type_lambda) (applied_constructor_type)
         (wildcard)])
(val_declaration
  name: (identifier) @ref.receiver.unproven
  type: [(stable_type_identifier) (generic_type type: (stable_type_identifier)) (singleton_type)
         (projected_type) (function_type) (compound_type) (infix_type) (annotated_type)
         (tuple_type) (named_tuple_type) (structural_type) (literal_type) (match_type)
         (type_lambda) (applied_constructor_type) (wildcard)])
(var_declaration
  name: (identifier) @ref.receiver.unproven
  type: [(stable_type_identifier) (generic_type type: (stable_type_identifier)) (singleton_type)
         (projected_type) (function_type) (compound_type) (infix_type) (annotated_type)
         (tuple_type) (named_tuple_type) (structural_type) (literal_type) (match_type)
         (type_lambda) (applied_constructor_type) (wildcard)])
(val_definition
  pattern: (identifier) @ref.receiver.unproven
  type: [(stable_type_identifier) (generic_type type: (stable_type_identifier)) (singleton_type)
         (projected_type) (function_type) (compound_type) (infix_type) (annotated_type)
         (tuple_type) (named_tuple_type) (structural_type) (literal_type) (match_type)
         (type_lambda) (applied_constructor_type) (wildcard)])
(var_definition
  pattern: (identifier) @ref.receiver.unproven
  type: [(stable_type_identifier) (generic_type type: (stable_type_identifier)) (singleton_type)
         (projected_type) (function_type) (compound_type) (infix_type) (annotated_type)
         (tuple_type) (named_tuple_type) (structural_type) (literal_type) (match_type)
         (type_lambda) (applied_constructor_type) (wildcard)])

; An own-field access `this.x` (the bound cohesion input). Over-capturing a
; `this.m()` method call here is harmless: with no Field named `m` in the class,
; the Accesses candidate simply never binds (NFR-RA-05).
(
  (field_expression
    value: (identifier) @_recv
    field: (identifier) @ref.access)
  (#eq? @_recv "this")
)

; Bare field access (S-575, FR-EX-28). `semaphore.acquire(n)`, `rate - n` and
; `s"$label"` read the enclosing class's own fields with no `this.` written.
; Only the positions a VALUE is spelled in are listed — a receiver, an
; argument, an operand, an assigned or returned expression, an expression body,
; a block's statement or result, an interpolation, a condition or branch, a
; scrutinee, a tuple element, a generator's source, an applied field
; (`items(0)`) — so a name in a TYPE or DECLARATION position, a member
; selected off another value (`o.count`) and a named argument's label
; (`copy(rate = 1)`, an assignment inside an argument list) are never
; captured. `collect_refs` records a row only where the method sits in a class
; that itself declares a field of that name (`bare_access`) — so a method, a
; type, an object and a name the class inherits record nothing — and drops it
; where the same method declares a local of the name (`@ref.access.shadow`
; below); the binder then proves an exactly-one Field in that class
; (NFR-RA-05). A field an outer class declares is not seen.
[
  (field_expression value: (identifier) @ref.access.bare)
  (call_expression function: (identifier) @ref.access.bare)
  (arguments (identifier) @ref.access.bare)
  (infix_expression left: (identifier) @ref.access.bare)
  (infix_expression right: (identifier) @ref.access.bare)
  (prefix_expression (identifier) @ref.access.bare)
  (parenthesized_expression (identifier) @ref.access.bare)
  (tuple_expression (identifier) @ref.access.bare)
  (ascription_expression . (identifier) @ref.access.bare)
  (assignment_expression right: (identifier) @ref.access.bare)
  (return_expression (identifier) @ref.access.bare)
  (interpolation (identifier) @ref.access.bare)
  (block (identifier) @ref.access.bare)
  (indented_block (identifier) @ref.access.bare)
  (function_definition body: (identifier) @ref.access.bare)
  (val_definition value: (identifier) @ref.access.bare)
  (var_definition value: (identifier) @ref.access.bare)
  (if_expression condition: (identifier) @ref.access.bare)
  (if_expression consequence: (identifier) @ref.access.bare)
  (if_expression alternative: (identifier) @ref.access.bare)
  (while_expression condition: (identifier) @ref.access.bare)
  (match_expression value: (identifier) @ref.access.bare)
  (case_clause body: (identifier) @ref.access.bare)
  (lambda_expression (identifier) @ref.access.bare .)
  (for_expression body: (identifier) @ref.access.bare)
  (enumerator (_) . (identifier) @ref.access.bare)
]
; An assignment's target is a read-write of the field (`count = count + 1`),
; but only as a statement: inside an argument list it is a named argument's
; label, which names a parameter.
(block (assignment_expression left: (identifier) @ref.access.bare))
(indented_block (assignment_expression left: (identifier) @ref.access.bare))
(function_definition body: (assignment_expression left: (identifier) @ref.access.bare))

; Names a method declares (S-575). A bare name the method declares is the
; local, not the class's field, so each declaring node marks its name and
; `collect_refs` drops the method's `@ref.access.bare` rows headed by it. The
; whole method is read as one scope, which can lose an access (a field read
; beside an unrelated lambda parameter of the same name) and never invents
; one. A class parameter is the field itself, so it is no shadow; a member
; `val`, `def` or `object` marks only its own name, under the member it is (a
; class declares no field of its member's name). A local `def`, `object` or
; `class` is a name the method declares, and so is each name of `val a, b = 0`.
(parameter name: (identifier) @ref.access.shadow)
(val_definition pattern: (identifier) @ref.access.shadow)
(var_definition pattern: (identifier) @ref.access.shadow)
(val_definition pattern: (identifiers (identifier) @ref.access.shadow))
(var_definition pattern: (identifiers (identifier) @ref.access.shadow))
(function_definition name: (identifier) @ref.access.shadow)
(object_definition name: (identifier) @ref.access.shadow)
(class_definition name: (identifier) @ref.access.shadow)
(tuple_pattern (identifier) @ref.access.shadow)
(case_clause pattern: (identifier) @ref.access.shadow)
(case_class_pattern pattern: (identifier) @ref.access.shadow)
(typed_pattern pattern: (identifier) @ref.access.shadow)
(capture_pattern name: (identifier) @ref.access.shadow)
(infix_pattern left: (identifier) @ref.access.shadow)
(infix_pattern right: (identifier) @ref.access.shadow)
(named_pattern (identifier) @ref.access.shadow)
(repeat_pattern pattern: (identifier) @ref.access.shadow)
(given_definition name: (identifier) @ref.access.shadow)
(lambda_expression parameters: (identifier) @ref.access.shadow)
(binding name: (identifier) @ref.access.shadow)
(enumerator . (identifier) @ref.access.shadow)

; An import declaration, walked into one row per imported path (S-518).
(import_declaration) @ref.import.dotted

; ── Argument count (S-591, CR-190, FR-EX-32) ─────────────────────────────────
; The `@arity.*` vocabulary the extraction engine reads (`extract::arity`): every
; argument list, whose named children a call row counts, and the forms that
; make a count unknown. Captures record no row of their own, so every ledger
; target is unchanged.
; A call counts its first argument list only; a block or case block written as
; the list (`f { … }`) is one argument. `xs: _*` and `xs*` spread a sequence.
(arguments) @arity.arguments
(call_expression
  arguments: [(block) (case_block)] @arity.block)
(call_expression
  arguments: (colon_argument) @arity.opaque)
(arguments (ascription_expression (repeated_parameter_type)) @arity.spread)
(arguments
  (postfix_expression (operator_identifier) @_spread) @arity.spread
  (#eq? @_spread "*"))
