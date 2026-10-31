; Kotlin reference-extraction query (S-055, capability = "references").
;
;   @ref.method — a call's callee name (`callee()`, `service.list()`); a
;                 Method-form row that binds by its receiver's SHAPE, recorded
;                 by the `@ref.receiver.*` markers below (S-514, S-516, CR-169).
;                 A receiver call no receiver type proves binds the one
;                 extension of its name in scope (S-560, `plugin.toml`'s
;                 `extension_functions`).
;   @ref.call   — a call on a class or object (S-560, FR-RS-31): `Ticket.empty()`
;                 recorded with the class (`@ref.call.scope`) as the Path
;                 `Ticket::empty` and bound among that type's members through the
;                 file's imports, its package and its wildcards. A receiver is
;                 read as a class when it is one capitalised name; a value of
;                 that spelling the file declares (`@ref.call.shadow`) drops the
;                 row instead.
;   @ref.import — an import declaration's qualified path
;                 (`org.springframework.web…`); canonicalised (dots → `::`)
;                 into the ledger form feeding the binder and the framework
;                 candidacy gate (FR-FW-04).
;   @ref.import.alias — the local name an `import a.b.C as D` binds (`D`;
;                 S-520). The row's alias is that name, not `C` — which the
;                 import does not bring into view, so a same-package `C` is
;                 still the one an unqualified `C` names; without an `as` it
;                 stays the path's last segment.
;   @ref.access — an own-property access (`this.x`): the bound LCOM4 input
;                 (Method → Field), the same structural pattern as Java's
;                 `this.<field>` access (CR-005, FR-EX-08).
;   @ref.access.bare — a bare name in a value position (`count`, the `total` of
;                 `"${total}"`): an `Accesses` row of the method when the enclosing
;                 class declares a property of that name and no local or
;                 parameter of the method does (S-562, FR-EX-25). See "Bare
;                 property access" below.
;   @ref.access.shadow — a name a method declares (a parameter, a local, a
;                 lambda, `for` or `catch` variable): it records no row; it
;                 drops the `@ref.access.bare` rows of that method headed by its
;                 name.
;   @ref.extends — each supertype a class, interface or object lists after `:`
;                 (`class A : Base(), Iface` → `Base`, `Iface`; S-522,
;                 FR-RS-15), whether invoked as the base class's constructor,
;                 delegated (`Iface by impl`) or bare. The list does not say
;                 which entry is the class, so the plugin declares
;                 `supertype_kind_follows_target`: a class's entry binds the one
;                 in-repository class or interface it names, through the
;                 package rungs, and its edge is `Extends` to a class,
;                 `Implements` to an interface. An interface's entries are its
;                 super-interfaces (`Extends`).
;
; Receiver-shape markers (S-516, FR-EX-13, FR-RS-12) — they record no row of
; their own; each names the receiver of the `@ref.method` that shares its parent
; node (the `navigation_expression`, or for a bare call the `call_expression`):
;
;   @ref.receiver.self      — `this.list()`: binds among the enclosing class's
;                             own members only. A labelled `this@Outer` names
;                             another instance, so it is `other`.
;   @ref.receiver.super     — `super.list()`: binds only through a proven
;                             `Extends` of the caller's class — its base class,
;                             never an interface (`@ref.extends` below).
;                             `super<T>.list()` names the supertype `T` and
;                             `super@Outer.list()` an outer class's, neither of
;                             which the caller's own hierarchy decides: both are
;                             `other`.
;   @ref.receiver.other     — every other receiver (`service.list()`,
;                             `a.b.list()`): never bound through the caller's
;                             scope (`no-receiver-evidence`).
;   @ref.receiver.implicit  — `list()`, no receiver: under this plugin's
;                             `implicit_receiver = "self"` a call on the current
;                             instance inside a class or object, a free call
;                             outside one.
;   @ref.receiver.anonymous — an `object : T { … }` body, and an extension
;                             function's body (`fun G.ext() { … }`, whose `this`
;                             is a `G`): its receiver has no node to bind
;                             through, so a `this.` call inside records `other`
;                             and a bare call is a free call.
;
; Droppable on disk at `.logos/plugins/kotlin/queries/references.scm`.
;
; Deliberately NOT captured: a call on a qualified class (`a.b.C.m()`,
; `Outer.Inner.m()`) — its receiver is not one name, so it stays a receiver call.

; A receiver-less call: `callee()` — the callee is the call's first expression
; child, here a bare identifier. (Arguments live under `value_arguments`, never
; as a direct identifier child, so this captures only the callee.)
(call_expression
  (identifier) @ref.method @ref.receiver.implicit)

; A receiver method call: `service.list()` — the member name after navigation.
; `navigation_expression` has no fields and its receiver may itself be a bare
; `identifier`, so the member is anchored as the navigation's LAST child: in
; `a.b.c()` only `c` is a call, never the receiver `a` (KOTLIN-G4). A receiver
; that is one capitalised name is a class or object: its call is the qualified
; `@ref.call` below.
((call_expression
  (navigation_expression
    .
    (_) @_receiver
    (identifier) @ref.method .))
  (#not-match? @_receiver "^[A-Z][A-Za-z0-9_]*$"))

; A call on a class or object (S-560, FR-RS-31): `Ticket.empty()` — a companion
; member — and `CacheZoneStat.init()` on an `object`, recorded with the class
; (`@ref.call.scope`) as the Path `Ticket::empty` and bound among that type's
; members — a class's companion's first (`plugin.toml`'s `companion_name`) —
; through the file's imports, its package and its wildcards, never through the
; caller's own class. A receiver is read as a class when it is one
; capitalised name; a value of that spelling the file declares
; (`@ref.call.shadow`) drops the row instead.
((call_expression
  (navigation_expression
    .
    (identifier) @ref.call.scope
    (identifier) @ref.call .))
  (#match? @ref.call.scope "^[A-Z][A-Za-z0-9_]*$"))

;   @ref.call.shadow — a capitalised value the file declares (a property, a
;                 parameter, a lambda parameter): a `Codec.length()` on it is a
;                 call on that value, so its class-qualified row is dropped
;                 (S-553's rule, read file-wide).
((variable_declaration
  (identifier) @ref.call.shadow)
  (#match? @ref.call.shadow "^[A-Z]"))
((parameter
  (identifier) @ref.call.shadow)
  (#match? @ref.call.shadow "^[A-Z]"))
; A constructor parameter's name is the identifier before its `:` — a default
; value after it may be an identifier too, which names nothing declared here.
((class_parameter
  (identifier) @ref.call.shadow
  ":")
  (#match? @ref.call.shadow "^[A-Z]"))

; The receiver of a member call: the navigation's first child.
((navigation_expression
  .
  (this_expression) @ref.receiver.self)
  (#eq? @ref.receiver.self "this"))

((navigation_expression
  .
  (super_expression) @ref.receiver.super)
  (#eq? @ref.receiver.super "super"))

(navigation_expression
  .
  (_) @ref.receiver.other)

(object_literal
  (class_body) @ref.receiver.anonymous)

; An extension function's body: its `this` is the extension receiver (`G` in
; `fun G.ext()`), not the enclosing class's instance, so a `this.` call inside
; it must not bind among that class's members. The receiver type sits right
; before the name; a return type follows the parameters, so the anchor tells
; the two apart.
(function_declaration
  [(user_type) (nullable_type)]
  .
  name: (identifier)
  (function_body) @ref.receiver.anonymous)

; Receiver typing (S-563, CR-178, FR-RS-32, FR-RS-10) — the scope-precise
; vocabulary Rust, TypeScript, Python and PHP speak (`extract::receiver`),
; recording no row of its own. Where the one binding of a receiver's name in
; scope at the call proves its type `T`, the call's Method row is retyped to
; the Path-form `T::find` of shape `other` — a call on an instance, never the
; written `T.find()`, which reaches `T`'s companion first — and bound among
; `T`'s own members, up its proven supertypes; failing that, as an untyped
; call is, to the one extension of the name in scope (S-560). Anything
; unproven keeps its `other` row (`no-receiver-evidence`):
;
;   @ref.receiver.variable — `repo.find()`, `repo?.find()`: a one-name
;                            receiver. A capitalised one is the class call
;                            `@ref.call` records.
;   @ref.receiver.binding  — each name a declaration binds, and where:
;                            `.scope` the callable a parameter belongs to, the
;                            lambda, `for`, `catch` or `when` whose variable it
;                            is;
;                            `.after` a local's block or lambda, after it;
;                            `.member` a property's class, object or companion
;                            — only where that class is the call's own, outside
;                            every anonymous body and every lambda
;                            `@ref.receiver.rebinds` names.
;   @ref.receiver.proof    — the binding's type: `.type` a declared `T` or `T?`
;                            (`T<A>` reads `T`), `.literal` `val x = T()`. A
;                            binding with no proof — `val x = make()`, an
;                            untyped lambda parameter, a destructured name —
;                            proves nothing, and hides every other binding of
;                            its name.
;   @ref.receiver.rebinds  — a lambda whose `this` may be another receiver
;                            than the enclosing class's instance, so a property
;                            the class declares may be that receiver's there:
;                            every lambda (`x.apply { }`, `with(x) { }`, a
;                            `T.() -> R` builder, one passed in parentheses or
;                            stored) except the trailing lambda of a standard
;                            function that takes none (`let`, `forEach`, `map`,
;                            … — listed below) and a bare `run { }` /
;                            `apply { }`, whose receiver is that instance.
;   @ref.receiver.unproven — `it`, whose implicit binding no pattern names, and
;                            a `vararg` parameter, an array of its declared
;                            type: never typed, anywhere in the file.
;
; Stated ceilings: two bindings of a name in scope prove nothing, so a
; parameter or local that shadows a property refuses the call rather than
; picking the nearer one; a property a supertype, an outer class or a
; companion declares is not in scope; a plain constructor parameter (no
; `val`/`var`) proves nothing, and hides a property of its name across the
; class — in an initialiser it is the one read; a top-level property is bound nowhere — every implicit
; receiver's member of its name (an inherited one, a companion's, an outer
; class's, an extension receiver's) outranks it, and none of those is read; `this.x.find()`, `x!!.find()`, a chain, a qualified type
; (`a.b.Repo`), a function type, a type parameter, `val x = T<A>()` and
; `val x = T { … }` prove nothing; a property read inside any lambda but the
; listed non-receiver ones is not typed.
((call_expression
  (navigation_expression
    .
    (identifier) @ref.receiver.variable
    (identifier) .))
  (#not-match? @ref.receiver.variable "^[A-Z][A-Za-z0-9_]*$"))

; Parameters: of a function, a secondary constructor, an anonymous function.
(_
  (function_value_parameters
    (parameter (identifier) @ref.receiver.binding))) @ref.receiver.binding.scope
(parameter (identifier) @ref.receiver.proof (user_type) @ref.receiver.proof.type)
(parameter (identifier) @ref.receiver.proof (nullable_type (user_type) @ref.receiver.proof.type))
((function_value_parameters
  (parameter_modifiers (parameter_modifier) @_vararg)
  .
  (parameter (identifier) @ref.receiver.unproven))
  (#eq? @_vararg "vararg"))

; Constructor parameters. The name is the identifier before the `:`; a default
; value after it may be an identifier too. Only a `val`/`var` one is a property
; a member reads, so only it carries a proof: a plain one is the name an
; initialiser or `init` block reads, and hides the property there.
(class_declaration
  (primary_constructor
    (class_parameters
      (class_parameter (identifier) @ref.receiver.binding ":")))) @ref.receiver.binding.member
(class_parameter ["val" "var"] (identifier) @ref.receiver.proof ":" (user_type) @ref.receiver.proof.type)
(class_parameter ["val" "var"] (identifier) @ref.receiver.proof ":" (nullable_type (user_type) @ref.receiver.proof.type))

; Properties of a class, an enum class, an object, a companion.
(class_declaration
  (class_body
    (property_declaration (variable_declaration (identifier) @ref.receiver.binding)))) @ref.receiver.binding.member
(class_declaration
  (enum_class_body
    (property_declaration (variable_declaration (identifier) @ref.receiver.binding)))) @ref.receiver.binding.member
(object_declaration
  (class_body
    (property_declaration (variable_declaration (identifier) @ref.receiver.binding)))) @ref.receiver.binding.member
(companion_object
  (class_body
    (property_declaration (variable_declaration (identifier) @ref.receiver.binding)))) @ref.receiver.binding.member

; Locals, from their declaration to the end of their block or lambda.
(block
  (property_declaration (variable_declaration (identifier) @ref.receiver.binding)) @ref.receiver.binding.after)
(block
  (property_declaration (multi_variable_declaration) @ref.receiver.binding) @ref.receiver.binding.after)
(lambda_literal
  (property_declaration (variable_declaration (identifier) @ref.receiver.binding)) @ref.receiver.binding.after)
(lambda_literal
  (property_declaration (multi_variable_declaration) @ref.receiver.binding) @ref.receiver.binding.after)

; Lambda parameters, loop, `catch` and `when` subject variables.
(lambda_literal
  (lambda_parameters (variable_declaration (identifier) @ref.receiver.binding))) @ref.receiver.binding.scope
(lambda_literal
  (lambda_parameters (multi_variable_declaration) @ref.receiver.binding)) @ref.receiver.binding.scope
(for_statement
  (variable_declaration (identifier) @ref.receiver.binding)) @ref.receiver.binding.scope
(for_statement
  (multi_variable_declaration) @ref.receiver.binding) @ref.receiver.binding.scope
(catch_block (identifier) @ref.receiver.binding) @ref.receiver.binding.scope
(catch_block (identifier) @ref.receiver.proof (user_type) @ref.receiver.proof.type)
(when_expression
  (when_subject (variable_declaration (identifier) @ref.receiver.binding))) @ref.receiver.binding.scope

; A declared type, wherever the declaration sits; `val x = T()` with none.
(variable_declaration (identifier) @ref.receiver.proof (user_type) @ref.receiver.proof.type)
(variable_declaration (identifier) @ref.receiver.proof (nullable_type (user_type) @ref.receiver.proof.type))
((property_declaration
  (variable_declaration . (identifier) @ref.receiver.proof .)
  (call_expression . (identifier) @ref.receiver.proof.literal . (value_arguments) .))
  (#match? @ref.receiver.proof.literal "^[A-Z]"))

; Every lambda but the trailing one of a function that takes no receiver
; lambda: `x.f { }`, `f { }`, `f(a) { }`, `x.f(a) { }` by the callee's name;
; one passed in parentheses, stored, assigned or returned always.
((call_expression
  (navigation_expression (identifier) @_lambda_callee .)
  (annotated_lambda (lambda_literal) @ref.receiver.rebinds))
  (#not-any-of? @_lambda_callee
    "let" "also" "takeIf" "takeUnless" "forEach" "forEachIndexed" "onEach" "map"
    "mapNotNull" "mapIndexed" "flatMap" "filter" "filterNot" "filterIndexed"
    "any" "all" "none" "count" "first" "firstOrNull" "last" "lastOrNull" "find"
    "single" "singleOrNull" "sortedBy" "sortedByDescending" "groupBy" "associate"
    "associateBy" "associateWith" "partition" "fold" "reduce" "sumOf"
    "maxByOrNull" "minByOrNull" "takeWhile" "dropWhile" "distinctBy" "use"
    "repeat" "lazy" "getOrElse" "getOrPut"))
((call_expression
  .
  (identifier) @_lambda_callee
  (annotated_lambda (lambda_literal) @ref.receiver.rebinds))
  (#not-any-of? @_lambda_callee "run" "apply"
    "let" "also" "takeIf" "takeUnless" "forEach" "forEachIndexed" "onEach" "map"
    "mapNotNull" "mapIndexed" "flatMap" "filter" "filterNot" "filterIndexed"
    "any" "all" "none" "count" "first" "firstOrNull" "last" "lastOrNull" "find"
    "single" "singleOrNull" "sortedBy" "sortedByDescending" "groupBy" "associate"
    "associateBy" "associateWith" "partition" "fold" "reduce" "sumOf"
    "maxByOrNull" "minByOrNull" "takeWhile" "dropWhile" "distinctBy" "use"
    "repeat" "lazy" "getOrElse" "getOrPut"))
((call_expression
  .
  (call_expression . (identifier) @_lambda_callee)
  (annotated_lambda (lambda_literal) @ref.receiver.rebinds))
  (#not-any-of? @_lambda_callee
    "let" "also" "takeIf" "takeUnless" "forEach" "forEachIndexed" "onEach" "map"
    "mapNotNull" "mapIndexed" "flatMap" "filter" "filterNot" "filterIndexed"
    "any" "all" "none" "count" "first" "firstOrNull" "last" "lastOrNull" "find"
    "single" "singleOrNull" "sortedBy" "sortedByDescending" "groupBy" "associate"
    "associateBy" "associateWith" "partition" "fold" "reduce" "sumOf"
    "maxByOrNull" "minByOrNull" "takeWhile" "dropWhile" "distinctBy" "use"
    "repeat" "lazy" "getOrElse" "getOrPut"))
((call_expression
  .
  (call_expression . (navigation_expression (identifier) @_lambda_callee .))
  (annotated_lambda (lambda_literal) @ref.receiver.rebinds))
  (#not-any-of? @_lambda_callee
    "let" "also" "takeIf" "takeUnless" "forEach" "forEachIndexed" "onEach" "map"
    "mapNotNull" "mapIndexed" "flatMap" "filter" "filterNot" "filterIndexed"
    "any" "all" "none" "count" "first" "firstOrNull" "last" "lastOrNull" "find"
    "single" "singleOrNull" "sortedBy" "sortedByDescending" "groupBy" "associate"
    "associateBy" "associateWith" "partition" "fold" "reduce" "sumOf"
    "maxByOrNull" "minByOrNull" "takeWhile" "dropWhile" "distinctBy" "use"
    "repeat" "lazy" "getOrElse" "getOrPut"))
(value_argument (lambda_literal) @ref.receiver.rebinds)
(property_declaration (lambda_literal) @ref.receiver.rebinds)
(assignment (lambda_literal) @ref.receiver.rebinds)
(function_body (lambda_literal) @ref.receiver.rebinds)
(return_expression (lambda_literal) @ref.receiver.rebinds)

((navigation_expression . (identifier) @ref.receiver.unproven)
  (#eq? @ref.receiver.unproven "it"))

; A bare call inside a class, object or interface is read among that type's
; members first (`@ref.receiver.implicit`, `self`). Where the file shows that no
; member can answer the name, it is the free call the imports and the package
; bind (S-560, FR-RS-31; S-573's `extract::free_call`): `enumMatcher(s)` inside a
; handler calls the imported top-level `mixit.util.enumMatcher`. Three marks,
; recording no row of their own:
;
;   @ref.implicit.free  — a bare call's name that may be free; `Any`'s members,
;                         which every instance has, are never marked.
;   @ref.implicit.open  — a body whose members the file cannot all see: one
;                         that inherits (a supertype list, an `object : T { … }`,
;                         a companion with a supertype)
;                         or one the compiler adds members to (a `data class`,
;                         an `enum class`). A call inside one stays on the
;                         instance.
;   @ref.implicit.value — a name the file binds as a value — a parameter, a
;                         constructor parameter, a property, a local, a lambda,
;                         `for` or `catch` variable: a call of it invokes that
;                         value, and never reaches a function of its name.
((call_expression
  (identifier) @ref.implicit.free)
  (#not-any-of? @ref.implicit.free "equals" "hashCode" "toString"))

(class_declaration
  (delegation_specifiers)
  [(class_body) (enum_class_body)] @ref.implicit.open)
(class_declaration
  (enum_class_body) @ref.implicit.open)
((class_declaration
  (modifiers (class_modifier) @_data)
  (class_body) @ref.implicit.open)
  (#eq? @_data "data"))
(object_declaration
  (delegation_specifiers)
  (class_body) @ref.implicit.open)
; A companion is an implicit receiver of its whole class: one that inherits
; opens the class's body, its own body included.
((class_body
  (companion_object
    (delegation_specifiers))) @ref.implicit.open)
(object_literal
  (class_body) @ref.implicit.open)

(parameter (identifier) @ref.implicit.value)
(class_parameter (identifier) @ref.implicit.value ":")
(variable_declaration (identifier) @ref.implicit.value)
(catch_block (identifier) @ref.implicit.value)

; An import's qualified path — canonicalised (dots → `::`) into the ledger form
; that feeds the binder and the Spring candidacy gate. `import a.b.C` (and its
; aliased form) names one declaration; `import a.b.*` brings every type of the
; package `a.b` into view, so it is marked a wildcard (`@ref.import.asterisk`,
; S-518) and records no alias. The two are told apart by the trailing `*`, so
; exactly one pattern matches each import. An aliased import's `identifier`
; after `as` is its local name (`@ref.import.alias`).
((import
  (qualified_identifier) @ref.import
  (identifier)? @ref.import.alias) @_import
  (#not-match? @_import "\\*\\s*;?\\s*$"))

(import
  (qualified_identifier) @ref.import
  "*" @ref.import.asterisk)

; An own-property access `this.x`: a method reading a property of its own class.
; A labelled `this@Outer.x` names another instance, so it is not captured.
; The class lexically Contains both the method and its `property` declarations
; (symbols.scm captures Kotlin properties as Field), so resolution binds an
; exactly-one Field candidate to an `Accesses` edge (Method → Field); an
; ambiguous access stays unresolved (NFR-RA-05). This is the bound LCOM4 input.
((navigation_expression
  (this_expression) @_this
  (identifier) @ref.access)
  (#eq? @_this "this"))

; Bare property access (S-562, FR-EX-25). `count + x`, `repo.find(id)` and
; `"${total}"` read the enclosing class's own properties with no `this.` written.
; Only the positions a VALUE is spelled in are listed — an operand, an argument,
; an assigned, returned or thrown expression, an expression body, a statement or
; result of a block or lambda, an interpolation, a condition, a branch, a subject,
; an iterated collection, an index, a spread, the receiver of a selection
; (`name.length`) — so a name in a TYPE or DECLARATION position, the member
; selected off another value (`o.count`), a call's callee (`count()`, which the
; `@ref.method` row records), a named argument's label (`copy(rate = 1)`), a
; `return@label x` and a callable reference (`::count`) are never captured. A
; short `"$total"` is string text to the grammar, so it is not read either.
; `collect_refs` records a row only where the method sits in a class that itself
; declares a property of that name (`bare_access`) — so a method, a type, an
; object and a name the class inherits record nothing — and drops it where the
; same method declares a local of the name (`@ref.access.shadow` below); the
; binder then proves an exactly-one Field in that class (NFR-RA-05). A property
; an outer class declares is not seen, nor one only the body of an `object : T`
; or an enum entry declares; and a name read inside such a body, an extension
; function, or the trailing lambda of `x.apply { }`, `x.run { }` or `with(x) { }`
; is not claimed — its receiver may declare it (`on_another_receiver`).
[
  (binary_expression left: (identifier) @ref.access.bare)
  (binary_expression right: (identifier) @ref.access.bare)
  (unary_expression argument: (identifier) @ref.access.bare)
  (is_expression left: (identifier) @ref.access.bare)
  (as_expression left: (identifier) @ref.access.bare)
  (in_expression left: (identifier) @ref.access.bare)
  (in_expression right: (identifier) @ref.access.bare)
  (range_expression (identifier) @ref.access.bare)
  (index_expression (identifier) @ref.access.bare)
  (infix_expression . (identifier) @ref.access.bare)
  (infix_expression (identifier) @ref.access.bare .)
  (parenthesized_expression (identifier) @ref.access.bare)
  (spread_expression (identifier) @ref.access.bare)
  (interpolation (identifier) @ref.access.bare)
  (value_argument . (identifier) @ref.access.bare .)
  (value_argument "=" . (identifier) @ref.access.bare)
  (assignment right: (identifier) @ref.access.bare)
  (property_declaration "=" . (identifier) @ref.access.bare)
  (return_expression "return" . (identifier) @ref.access.bare)
  (throw_expression (identifier) @ref.access.bare)
  (function_body (identifier) @ref.access.bare)
  (block (identifier) @ref.access.bare)
  (lambda_literal (identifier) @ref.access.bare)
  (if_expression (identifier) @ref.access.bare)
  (when_subject (identifier) @ref.access.bare)
  (when_entry (identifier) @ref.access.bare)
  (while_statement (identifier) @ref.access.bare)
  (do_while_statement (identifier) @ref.access.bare)
  (for_statement (identifier) @ref.access.bare)
  (navigation_expression . (identifier) @ref.access.bare)
]
; An assignment's target is a read-write of the property (`count = count + 1`,
; `name += x`), as `this.count = …` is.
(assignment left: (identifier) @ref.access.bare)

; Names a method declares (S-562). A bare name the method declares is the local,
; not the class's property, so each declaring node marks its name and
; `collect_refs` drops the method's `@ref.access.bare` rows headed by it. The
; whole method is read as one scope, which can lose an access (a property read
; beside an unrelated lambda parameter of the same name) and never invents one.
; A constructor parameter is no shadow: a `val` one is the property itself, and
; a plain one is not in scope in a method body.
(parameter (identifier) @ref.access.shadow)
; A local `val`/`var`, a destructured name, a lambda parameter and a `for`
; variable are each a `variable_declaration`.
(variable_declaration (identifier) @ref.access.shadow)
(catch_block (identifier) @ref.access.shadow)

;   @ref.extends — anchored to the declarations that own a supertype list. An
;   object expression's (`object : T { … }`) has no node of its own, so it is
;   never read as the enclosing declaration's. A companion is a node (S-559), but
;   its supertype list is not anchored here: a member it inherits is not reached
;   through it (I4-INT).
(class_declaration
  (delegation_specifiers
    (delegation_specifier
      [(user_type) @ref.extends
       (constructor_invocation (user_type) @ref.extends)
       (explicit_delegation (user_type) @ref.extends)])))
(object_declaration
  (delegation_specifiers
    (delegation_specifier
      [(user_type) @ref.extends
       (constructor_invocation (user_type) @ref.extends)
       (explicit_delegation (user_type) @ref.extends)])))

; ── Argument count (S-591, CR-190, FR-EX-32) ─────────────────────────────────
; The `@arity.*` vocabulary the extraction engine reads (`extract::arity`): every
; argument list, whose named children a call row counts, and the forms that
; make a count unknown. Captures record no row of their own, so every ledger
; target is unchanged.
; A trailing lambda (`f(1) { … }`, `f { … }`) is one more argument; `*xs`
; spreads an array into a vararg.
(value_arguments) @arity.arguments
(annotated_lambda) @arity.lambda
(value_arguments (value_argument (spread_expression)) @arity.spread)
