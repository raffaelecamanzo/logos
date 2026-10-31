; Kotlin symbol-extraction query (S-055, capability = "symbols").
;
; Capture names map to NodeKind via NodeKind::as_str (extract engine).
; Compiled against the built Kotlin Language on its first use, or at
; load as an on-disk override; drift fails naming this file (FR-PL-02,
; CR-197). Droppable on disk at
; `.logos/plugins/kotlin/queries/symbols.scm` (FR-PL-04, UAT-PL-03).
;
; class-bearing applicability (FR-QM-11): mapping a class construct to
; `@symbol.class` (NodeKind::Class) is the whole declaration — "the kind a
; construct extracts to is its declared applicability" (metrics::extended). So
; Cohesion/Focus apply to Kotlin classes, objects and companion objects, whose
; member `fun`s are Methods (below), and the field-sharing LCOM4 premise holds.
; An `enum class` is an Enum, which — as in Java and Rust — is no cohesion
; container.
;
; Kotlin's `class_declaration` covers `class`, `interface`, and `enum class`;
; the anonymous `class`/`interface` keyword token disjointly partitions the
; first two so no declaration is double-captured. An `enum class` carries the
; `class` token (plus an `enum` modifier), so the class pattern names it too:
; the `@decl.enum` marker below makes it an Enum (extract::definition). Both
; kinds render a type descriptor, so no symbol changes.

; A class (or enum class) → Class; an enum class is re-kinded by its marker.
(class_declaration
  "class"
  name: (identifier) @symbol.class)

; An `enum class` → Enum (S-559, FR-EX-24).
(class_declaration
  (modifiers (class_modifier "enum"))
  "class") @decl.enum

; An interface (incl. `fun interface`) → Interface.
(class_declaration
  "interface"
  name: (identifier) @symbol.interface)

; An `object` singleton → Class (a class-like stateful container).
(object_declaration
  name: (identifier) @symbol.class)

; A `companion object` → Class, a node of its own that holds its members
; (S-559, FR-EX-24): `Svc#Companion#create().`, which is where
; `Svc.Companion.create()` and `import …Svc.Companion.create` point. A named
; one takes its name; an anonymous one is `Companion`, which the grammar writes
; as no name — the `companion` keyword is the node captured and the setting
; names it.
(companion_object
  name: (identifier) @symbol.class)
(companion_object
  !name
  "companion" @symbol.class
  (#set! symbol.name "Companion"))

; A `typealias` → TypeAlias (S-559, FR-EX-24). The grammar's field for the
; alias name is `type`, which `Foo<T> = …`'s own type parameters follow.
(type_alias
  type: (identifier) @symbol.type_alias)

; A `fun` → Function, unless it is written in a class, object, interface or
; companion body (the marker below). Kotlin models member and free functions
; with one node, and a second pattern naming a member would name the same
; declaration twice, the first taken whichever the query lists first — so
; every `fun` is named here and a member is re-kinded by its marker
; (extract::definition). Function and Method render one descriptor, so the
; symbol is the same either way. A function in a function body stays a
; Function: a local, not a member.
(function_declaration
  name: (identifier) @symbol.function)

; A member `fun` → Method (S-559, FR-EX-24): a direct child of the body of a
; class, interface, object, companion object or `enum class`. That is what lets
; Cohesion and Focus count Kotlin's members (metrics::extended). The body of an
; object expression (`object : Runnable { … }`) and of an enum entry
; (`A { … }`) is a `class_body` too, but belongs to no declared type: their
; `fun`s stay Functions, as they were, and count toward no container.
(class_declaration
  (class_body
    (function_declaration) @decl.method))
(object_declaration
  (class_body
    (function_declaration) @decl.method))
(companion_object
  (class_body
    (function_declaration) @decl.method))
(enum_class_body
  (function_declaration) @decl.method)

; An extension `fun String.toSlug()` (S-560, FR-RS-31) is captured once more,
; with `@symbol.self_type` on its receiver type's name — the type written right
; before the function's name, `T?` unwrapped, and its last segment when written
; qualified (`fun a.b.Module.single()` is `Module`'s) — as Go's receiver is
; (`plugin.toml`'s `extension_functions`). A receiver call names it only when it
; records one. A return type follows the parameters, so the anchor tells the two
; apart.
(function_declaration
  (user_type (identifier) @symbol.self_type .)
  .
  name: (identifier) @symbol.function)
(function_declaration
  (user_type (identifier) @symbol.self_type . (type_arguments) .)
  .
  name: (identifier) @symbol.function)
(function_declaration
  (nullable_type (user_type (identifier) @symbol.self_type .))
  .
  name: (identifier) @symbol.function)
(function_declaration
  (nullable_type (user_type (identifier) @symbol.self_type . (type_arguments) .))
  .
  name: (identifier) @symbol.function)

; A `private` interface `fun` (S-609, CR-202, FR-RS-48): a class implementing
; the interface does not inherit it. The marker keeps it out of the supertype
; walk's interface levels (`plugin.toml`'s `inherits_interface_bodies`); it
; stays the interface's member. An abstract interface `fun` needs no marker — it
; records no body.
(class_declaration
  "interface"
  (class_body
    (function_declaration
      (modifiers (visibility_modifier "private"))) @item.uninherited))

; Nor does it inherit the interface's `companion object` functions: they are
; the interface's companion's members, and Kotlin links only a superCLASS's
; companion scope into a subclass, never a super-interface's.
(class_declaration
  "interface"
  (class_body
    (companion_object
      (class_body
        (function_declaration) @item.uninherited))))

; A class property → Field — the bound LCOM4 field-sharing input read by
; `this.<name>` accesses (references.scm). Scoped to `class_body` so only true
; member properties are captured: Kotlin reuses `property_declaration` for
; function-local `val`/`var` and top-level properties too, which are NOT fields
; and must not pollute the graph (the same boundary Java's class-only
; `field_declaration` gets for free). `class_body` is shared by `class`,
; `object`, and companion objects, so their properties are all covered; only
; `enum class` bodies (rarely property-bearing) fall outside, an accepted gap.
(class_body
  (property_declaration
    (variable_declaration
      (identifier) @symbol.field)))

; A primary-constructor `val`/`var` parameter → Field (S-559, FR-EX-24): a
; property of its class, declared where the class is. A plain parameter
; (`c: Int`) is no property and names none. The binding keyword precedes the
; name, which is the first identifier after it — a default value that is itself
; an identifier (`val a: Int = DEFAULT`) follows the type.
(class_parameter
  ["val" "var"]
  .
  (identifier) @symbol.field)

; An annotated declaration the grammar read as an expression (S-559, FR-EX-24;
; `damage` is no `symbol`: nothing is declared here). `tree-sitter-kotlin-ng`
; 1.1 reads a type declaration carrying two or more annotations, with another
; declaration after it, as an annotated expression, and an annotation with
; arguments (`@OptIn(X::class)`) apart from the declaration it precedes — and
; `has_error()` stays false, so the declaration or its annotations are lost
; with no error to report (frameworks.scm). Two shapes, both at the top of a
; file, are counted into the file's partial-extraction warning:
;   - an annotated expression the declaration that follows it is the other half
;     of (`@OptIn(X::class)` then `class C`, a `fun`, an `object` or a
;     `typealias`);
;   - one that holds the swallowed declaration whole, read as an infix call
;     (`@A @B class C { … }` is `class` applying `C` to a lambda; a modifier
;     such as `open` or `expect` nests one infix more), under up to three
;     annotations.
; An annotated expression with neither is not counted: it is what a Gradle
; `.kts` script's `@Suppress("x") plugins { … }` is, a statement and no loss.
(source_file
  (annotated_expression) @damage.misread
  .
  [(class_declaration) (object_declaration) (function_declaration) (type_alias)])
(source_file
  [(annotated_expression (infix_expression))
   (annotated_expression (annotated_expression (infix_expression)))
   (annotated_expression (annotated_expression (annotated_expression (infix_expression))))] @damage.misread)

; The file's `package` header (S-472, S-518, FR-RS-13). Not a declaration — its
; capture group is `module`, not `symbol`, so the declaration walk skips it. It is
; the file's module identity: each top-level type is named that package plus its
; own name, whatever source set or directory the file sits in (`commonMain`,
; `src/main/java`), so a Multiplatform file's imports bind like a JVM one's.
(package_header
  (qualified_identifier) @module.namespace)

; ── Parameter range (S-591, CR-190, FR-EX-32) ────────────────────────────────
; The `@arity.*` vocabulary the extraction engine reads (`extract::arity`): a
; callable's parameter list, and each parameter as required, optional (a
; default value raises only the maximum), variadic (an unbounded maximum), a
; receiver (not counted) or no parameter at all (skip). A list child no capture
; covers, or one captured `@arity.unknown`, records the range unknown rather
; than miscounting it. Captures never name a declaration, so every symbol and
; node is unchanged.
; A default value is written beside its parameter (`b: Int = 2`): the parameter
; before `=` is optional and the value after it no parameter. `vararg` is a
; modifier written before its parameter.
(function_declaration
  (function_value_parameters) @arity.parameters)
(function_value_parameters (parameter) @arity.required)
(function_value_parameters (parameter) @arity.optional . "=")
(function_value_parameters "=" . (_) @arity.skip)
(function_value_parameters
  (parameter_modifiers (parameter_modifier "vararg")) . (parameter) @arity.variadic)
(function_value_parameters (parameter_modifiers) @arity.skip)
; An `override fun` inherits the default values of the function it overrides,
; which its own list never writes (`override fun drop(scope: Scope?)` under
; `abstract fun drop(scope: Scope? = null)` admits `drop()`): its range is
; unknown (S-592).
(function_declaration
  (modifiers (member_modifier "override"))
  (function_value_parameters) @arity.unknown)
