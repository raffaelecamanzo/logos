; C# reference-extraction query (S-057, CR-009, capability = "references").
;
;   @ref.method — a method invocation's name (`service.List()`, `List()`, the
;                 generic `List<T>()` and the conditional `service?.List()`); a
;                 Method-form row that binds by its receiver's SHAPE, recorded
;                 by the `@ref.receiver.*` markers below (S-514, S-516, CR-169)
;                 — or, where the file proves its receiver's type `T`, the
;                 Path row `T::List` (S-558, "Receiver typing" below).
;                 A generic name is captured whole (`List<T>`) so it shares its
;                 parent with the receiver markers; `collect_refs` records its
;                 identifier alone (S-553).
;   @ref.call   — a call on a type name (`Helpers.Run()`, `Helpers.Make<T>()`):
;                 the Path-form `Helpers::Run`, bound among the members of the
;                 one type `Helpers` names in scope (FR-RS-28, FR-RS-13) and
;                 unbound where it names none or several. See "Calls on a type
;                 name" below.
;   @ref.call.shadow — a PascalCase value the file declares (a property, field,
;                 parameter, local, type parameter): records no row; it drops the
;                 `@ref.call` rows headed by its name (see "Values that obscure
;                 a type name" below).
;   @ref.instantiates — the type of `new T(…)`: an `Instantiates` row to the one
;                 class `T` names, the way Java's does (S-553, FR-RS-28). A
;                 struct, delegate or enum is no class, so its `new` binds
;                 nothing.
;   @ref.import — a `using` directive's namespace path (`Microsoft.AspNetCore.Mvc`);
;                 canonicalised (dots → `::`) into the ledger form feeding the
;                 binder and the framework candidacy gate (FR-FW-04). A plain
;                 `using N;` brings every type of `N` into view, so it is marked
;                 a wildcard (`@ref.import.asterisk`, S-518, FR-RS-13); see the
;                 `using` section below for `global`, `static` and aliases.
;   @ref.access — an own-field access (`this.X`): a method reading a field of its
;                 own type (CR-005, FR-EX-08); the bound LCOM4 input.
;   @ref.access.bare — a bare name in a value position (`_count`, `Name`, the
;                 receiver of `_log.Add(x)`): an `Accesses` row of the method
;                 when the enclosing class declares a field or property of that
;                 name and no local or parameter of the method does (S-556,
;                 FR-EX-22). See "Bare field and property access" below.
;   @ref.access.shadow — a name a method declares (a parameter, a local, a
;                 pattern or `out var` variable, a loop or catch variable, a
;                 lambda parameter, a query range variable): it records no row;
;                 it drops the `@ref.access.bare` rows of that method headed by
;                 its name.
;   @ref.extends — each entry of a class's, struct's, record's or interface's
;                 `base_list` (`class A : B, IC` → `B`, `IC`; S-522, FR-RS-15).
;                 The list does not say which entry is the base class, so the
;                 plugin declares `supertype_kind_follows_target`: a type's
;                 entry binds the one in-repository class or interface it names,
;                 through the namespace rungs, and its edge is `Extends` to a
;                 class, `Implements` to an interface. An interface's entries
;                 are its base interfaces (`Extends`). A generic base binds by
;                 its name (`JsonConverter<T>` → `JsonConverter`); `global::`
;                 makes it fully qualified.
;   @ref.extends.partial — a MARKER on a `partial` type declaration (S-557,
;                 FR-RS-29): the declaration's own name is captured `@ref.extends`
;                 in the same match, so each part records an `Extends` row naming
;                 itself, aliased `partial`. The parts sharing one
;                 fully-qualified name in one directory tree (each in or below
;                 one part's directory, standing in for the project) are one
;                 type: each later part's row binds to the first-declared part,
;                 by file path order, and the first part's own row stays unbound.
;                 Parts of another arity (`Box<T>` beside `Box`) are another type.
;
; Receiver-shape markers (S-516, FR-EX-13, FR-RS-12) — they record no row of
; their own; each names the receiver of the `@ref.method` that shares its parent
; node (the `member_access_expression`, for a bare call the invocation, and for a
; conditional call the name node itself):
;
;   @ref.receiver.self     — `this.List()`: binds among the enclosing class's
;                            own members only.
;   @ref.receiver.super    — `base.List()`: binds only through a proven
;                            `Extends` of the caller's type — its base class,
;                            never an interface (`@ref.extends` below).
;   @ref.receiver.other    — every other receiver (`service.List()`,
;                            `string.Join()`, `System.Console.Read()`): never
;                            bound through the caller's scope — typed where
;                            the one binding of its name in scope at the call
;                            proves its type (S-558, S-823, below), else
;                            `no-receiver-evidence`. A call on a type name
;                            (`Enumerable.Range()`) is no receiver call: it is
;                            the Path row `@ref.call` records.
;   @ref.receiver.implicit — `List()`, no receiver: under this plugin's
;                            `implicit_receiver = "self"` a call on the current
;                            instance inside a type, a free call outside one.
;
; Droppable on disk at `.logos/plugins/c-sharp/queries/references.scm`.
;
; Deliberately NOT captured: `using static` member binding, generic type
; arguments, a target-typed `new()` (its type is the declaration's, which no
; pattern here reads), and a call on a type name through a qualified or a
; predefined receiver (`A.B.Run()`, `string.Join()`).

; Bare call (`List()`, `List<T>()`): no receiver written.
(invocation_expression
  function: [(identifier) (generic_name)] @ref.method @ref.receiver.implicit)

; Calls on a type name (S-553, FR-RS-28). `Helpers.Run()` is told from
; `helpers.Run()` by the receiver's spelling alone — a PascalCase identifier is
; a type name by the language's own convention; the binder decides whether it
; names a type (exactly one, through the file's namespace, `using`s and
; enclosing namespaces) and binds nothing where it names none, as for any
; variable that happens to be spelled so. The capture is the whole
; `Helpers.Run` access, which `collect_refs` reads as the Path `Helpers::Run` —
; a generic name's type arguments dropped — and it is the call's only row: the
; Method-form patterns below exclude exactly this receiver.
(invocation_expression
  function: (member_access_expression
    expression: (identifier) @_type
    name: [(identifier) (generic_name)]) @ref.call
  (#match? @_type "^[A-Z][A-Za-z0-9_]*$"))

; Member call (`service.List()`, `this.List()`, `base.List()`): the method name.
; Any receiver but a type name, whose Path row is above.
(invocation_expression
  function: (member_access_expression
    expression: (_) @_receiver
    name: [(identifier) (generic_name)] @ref.method)
  (#not-match? @_receiver "^[A-Z][A-Za-z0-9_]*$"))

(invocation_expression
  function: (member_access_expression
    expression: ["this" "base"]
    name: [(identifier) (generic_name)] @ref.method))

; Values that obscure a type name (S-553). `Logger.Log()` on an `IAuditSink
; Logger { get; }` is a call on the property, not on a class `Logger`, and `T.Parse()`
; on a type parameter `T` is not one on a class `T`. Each declaration of a
; PascalCase value in the file — a property, field or local, a parameter, a
; `foreach`, `catch` or pattern variable, a type parameter — marks its name, and
; `collect_refs` drops a `@ref.call` row headed by a marked name. They record no
; row of their own. The file is read as a whole, which can lose a binding (a
; same-named type in a sibling class) and never invents one; a value inherited
; from a base class in another file is not seen.
(property_declaration name: (identifier) @ref.call.shadow (#match? @ref.call.shadow "^[A-Z]"))
(variable_declarator name: (identifier) @ref.call.shadow (#match? @ref.call.shadow "^[A-Z]"))
(parameter name: (identifier) @ref.call.shadow (#match? @ref.call.shadow "^[A-Z]"))
(foreach_statement left: (identifier) @ref.call.shadow (#match? @ref.call.shadow "^[A-Z]"))
(catch_declaration name: (identifier) @ref.call.shadow (#match? @ref.call.shadow "^[A-Z]"))
(declaration_pattern name: (identifier) @ref.call.shadow (#match? @ref.call.shadow "^[A-Z]"))
(type_parameter name: (identifier) @ref.call.shadow (#match? @ref.call.shadow "^[A-Z]"))

; Conditional call (`service?.List()`, `service?.List<T>()`): the callee is a
; `member_binding_expression`, whose receiver is the access's condition — a
; member call on another object, shape `other`. The marker shares the name's
; node, as the bare call's `implicit` one does; the condition is not a node the
; binding expression holds.
(invocation_expression
  function: (conditional_access_expression
    (member_binding_expression
      name: [(identifier) (generic_name)] @ref.method @ref.receiver.other)))

; `new T(…)` (S-553): the constructed type. A generic one records its name
; alone, a type parameter and a primitive record nothing, and the row binds to
; the class — constructors are not nodes (symbols.scm).
(object_creation_expression
  type: (_) @ref.instantiates)

; The receiver of a member call. `this` and `base` are anonymous keyword tokens
; in tree-sitter-c-sharp, matched as strings; `(_)` matches named nodes only,
; so the catch-all never reaches either keyword.
(member_access_expression
  expression: "this" @ref.receiver.self)

(member_access_expression
  expression: "base" @ref.receiver.super)

(member_access_expression
  expression: (_) @ref.receiver.other)

; Receiver typing (S-558, CR-177, FR-RS-30, FR-RS-10; scope-precise since S-823,
; CR-256) — the vocabulary Rust, TypeScript, Python, PHP, Kotlin and C++ speak
; (`extract::receiver`), recording no row of its own. Where the one binding of a
; member call's receiver name in scope at the call proves its type `T`, its
; Method row is retyped to the Path-form `T::List`, bound among `T`'s members
; and up its proven supertypes; anything unproven keeps its `other` row
; (`no-receiver-evidence`):
;
;   @ref.receiver.variable — `service.List()`, `_service.List()`: a one-name
;                            receiver. A PascalCase one is the type-name Path
;                            row above, never this marker.
;   @ref.receiver.binding  — each name a declaration binds, and where:
;                            `.scope` the callable a parameter belongs to (a
;                            method, constructor, operator, indexer, local
;                            function, lambda or anonymous method), the `for`,
;                            `foreach`, `using`, `fixed`, `catch`, `case` or
;                            switch arm whose variable it is;
;                            `.after` a local's block, after its statement;
;                            `.scope` a `case` section's local's whole switch
;                            body;
;                            `.member` a field, event or property's type, in
;                            that type's own members (never a nested type's);
;                            `.beneath` a primary-constructor parameter, in its
;                            type's own members beneath every other binding of
;                            its name, and nowhere in a type with a base list or
;                            a `partial` one, whose inherited members or other
;                            parts may hide it.
;   @ref.receiver.proof    — the binding's type: `.type` a declared `T`, `T<A>`
;                            (read `T`) or `T?`, `.literal` `var x = new T()`.
;                            A binding with no proof — `var x = Make()`, an
;                            untyped lambda parameter, a qualified (`A.B.T`,
;                            `global::T`) or `dynamic` type — proves nothing,
;                            and hides every other binding of its name.
;   @ref.receiver.flows    — a pattern or `out` variable no binding above
;                            states: in scope from where it is written to the
;                            end of the innermost @ref.receiver.flow_scope
;                            around it (a block, `case` section, callable,
;                            member declaration, or a loop, resource, `catch`,
;                            query, switch arm or embedded statement that
;                            scopes it to itself), keeping its proof.
;   @ref.receiver.hides    — every name a declaration declares. One no binding
;                            above states — a deconstructed, nested-pattern or
;                            query range name — is bound with no proof across
;                            the innermost @ref.receiver.hiding_scope (a
;                            callable, a property, a field declaration, a
;                            top-level statement), so an outer binding is never
;                            the one in scope there.
;   @ref.receiver.field    — `this.x.List()`: the field or property `x` the
;                            enclosing type declares itself.
;   @ref.receiver.declared — a field or property declared `T?`, whose `.type`
;                            companion `T` the `field` marker reads.
;   @ref.receiver.unproven — `value`, an accessor's implicit parameter, which no
;                            parameter list declares: never typed, anywhere in
;                            the file.
;
; Stated ceilings: two bindings of a name in scope prove nothing, so a
; parameter or local that shadows a field refuses the call rather than picking
; the nearer one; a field or property the type inherits, an outer type
; declares, or another part of a `partial` type declares is not in scope; a
; conditional call `x?.List()` is not typed; `var x = Make()` proves nothing;
; a top-level statement's local types no call; `o is var (a, b)` parses as a
; call on `o is var` (tree-sitter-c-sharp 0.23), so its names hide no field of
; theirs.
(member_access_expression
  expression: (identifier) @ref.receiver.variable
  (#not-match? @ref.receiver.variable "^[A-Z][A-Za-z0-9_]*$"))

; Parameters: of every callable that field-names its list `parameters`.
(_
  parameters: (parameter_list (parameter name: (identifier) @ref.receiver.binding))) @ref.receiver.binding.scope
(indexer_declaration
  parameters: (bracketed_parameter_list (parameter name: (identifier) @ref.receiver.binding))) @ref.receiver.binding.scope
(lambda_expression parameters: (implicit_parameter) @ref.receiver.binding) @ref.receiver.binding.scope

; Primary-constructor parameters.
(class_declaration
  (parameter_list (parameter name: (identifier) @ref.receiver.binding))) @ref.receiver.binding.beneath
(struct_declaration
  (parameter_list (parameter name: (identifier) @ref.receiver.binding))) @ref.receiver.binding.beneath
(record_declaration
  (parameter_list (parameter name: (identifier) @ref.receiver.binding))) @ref.receiver.binding.beneath

; Fields, events and properties of a class, struct, record or interface.
(_
  body: (declaration_list
    (field_declaration
      (variable_declaration (variable_declarator name: (identifier) @ref.receiver.binding))))) @ref.receiver.binding.member
(_
  body: (declaration_list
    (event_field_declaration
      (variable_declaration (variable_declarator name: (identifier) @ref.receiver.binding))))) @ref.receiver.binding.member
(_
  body: (declaration_list
    (property_declaration name: (identifier) @ref.receiver.binding))) @ref.receiver.binding.member

; Locals, from their statement to the end of their block — and a `case`
; section's local across the whole switch body, every section of which it is
; in scope in (a read before it is CS0841). A top-level statement's local is
; no binding: the byte range after it would reach the type declarations that
; follow, whose members never read it (CS8801).
(block
  (local_declaration_statement
    (variable_declaration (variable_declarator name: (identifier) @ref.receiver.binding))) @ref.receiver.binding.after)
(switch_body
  (switch_section
    (local_declaration_statement
      (variable_declaration (variable_declarator name: (identifier) @ref.receiver.binding))))) @ref.receiver.binding.scope

; Loop, resource, `catch`, `case` and switch-arm variables: their construct.
(for_statement
  initializer: (variable_declaration (variable_declarator name: (identifier) @ref.receiver.binding))) @ref.receiver.binding.scope
(foreach_statement left: (identifier) @ref.receiver.binding) @ref.receiver.binding.scope
(using_statement
  (variable_declaration (variable_declarator name: (identifier) @ref.receiver.binding))) @ref.receiver.binding.scope
(fixed_statement
  (variable_declaration (variable_declarator name: (identifier) @ref.receiver.binding))) @ref.receiver.binding.scope
(catch_clause (catch_declaration name: (identifier) @ref.receiver.binding)) @ref.receiver.binding.scope
(switch_section (declaration_pattern name: (identifier) @ref.receiver.binding)) @ref.receiver.binding.scope
(switch_expression_arm (declaration_pattern name: (identifier) @ref.receiver.binding)) @ref.receiver.binding.scope

; Pattern and `out` variables: from where each is written to the end of the
; innermost flow scope around it. C# keeps one declared in the condition of an
; `if` that sits in a block, or in an expression, declaration or `return`
; statement, in scope to the end of that block (`if (!(o is T p)) return;
; p.Run();`); one declared in a loop's condition, a `using`, `lock` or `fixed`
; resource, a `foreach` expression, a `catch` filter, a query clause, a switch
; arm or an embedded statement (`else if (…)`, an unbraced loop body) only in
; that construct. A `case` or switch-arm pattern is that construct's binding
; above.
(declaration_pattern name: (identifier) @ref.receiver.flows)
(declaration_expression name: (identifier) @ref.receiver.flows)
(block) @ref.receiver.flow_scope
(switch_section) @ref.receiver.flow_scope
(_ parameters: (_)) @ref.receiver.flow_scope
(arrow_expression_clause) @ref.receiver.flow_scope
(property_declaration) @ref.receiver.flow_scope
(field_declaration) @ref.receiver.flow_scope
(event_field_declaration) @ref.receiver.flow_scope
(global_statement) @ref.receiver.flow_scope
(while_statement) @ref.receiver.flow_scope
(do_statement) @ref.receiver.flow_scope
(for_statement) @ref.receiver.flow_scope
(foreach_statement) @ref.receiver.flow_scope
(using_statement) @ref.receiver.flow_scope
(lock_statement) @ref.receiver.flow_scope
(fixed_statement) @ref.receiver.flow_scope
(catch_clause) @ref.receiver.flow_scope
(query_expression) @ref.receiver.flow_scope
(switch_expression_arm) @ref.receiver.flow_scope
(if_statement consequence: (_) @ref.receiver.flow_scope)
(if_statement alternative: (_) @ref.receiver.flow_scope)

; A declared type, wherever the declaration sits; `var x = new T()`.
(variable_declaration
  type: [(identifier) (generic_name) (predefined_type)] @ref.receiver.proof.type
  (variable_declarator name: (identifier) @ref.receiver.proof)
  (#not-match? @ref.receiver.proof.type "^dynamic$"))
(variable_declaration
  type: (nullable_type type: [(identifier) (generic_name) (predefined_type)] @ref.receiver.proof.type)
  (variable_declarator name: (identifier) @ref.receiver.proof))
(variable_declaration
  type: (implicit_type)
  (variable_declarator
    name: (identifier) @ref.receiver.proof
    (object_creation_expression
      type: [(identifier) (generic_name)] @ref.receiver.proof.literal)))
(parameter
  type: [(identifier) (generic_name) (predefined_type)] @ref.receiver.proof.type
  name: (identifier) @ref.receiver.proof
  (#not-match? @ref.receiver.proof.type "^dynamic$"))
(parameter
  type: (nullable_type type: [(identifier) (generic_name) (predefined_type)] @ref.receiver.proof.type)
  name: (identifier) @ref.receiver.proof)
(property_declaration
  type: [(identifier) (generic_name) (predefined_type)] @ref.receiver.proof.type
  name: (identifier) @ref.receiver.proof
  (#not-match? @ref.receiver.proof.type "^dynamic$"))
(property_declaration
  type: (nullable_type type: [(identifier) (generic_name) (predefined_type)] @ref.receiver.proof.type)
  name: (identifier) @ref.receiver.proof)
(foreach_statement
  type: [(identifier) (generic_name) (predefined_type)] @ref.receiver.proof.type
  left: (identifier) @ref.receiver.proof
  (#not-match? @ref.receiver.proof.type "^dynamic$"))
(foreach_statement
  type: (nullable_type type: [(identifier) (generic_name) (predefined_type)] @ref.receiver.proof.type)
  left: (identifier) @ref.receiver.proof)
(catch_declaration
  type: [(identifier) (generic_name)] @ref.receiver.proof.type
  name: (identifier) @ref.receiver.proof)
(declaration_pattern
  type: [(identifier) (generic_name) (predefined_type)] @ref.receiver.proof.type
  name: (identifier) @ref.receiver.proof
  (#not-match? @ref.receiver.proof.type "^dynamic$"))
(declaration_expression
  type: [(identifier) (generic_name) (predefined_type)] @ref.receiver.proof.type
  name: (identifier) @ref.receiver.proof
  (#not-match? @ref.receiver.proof.type "^dynamic$"))
(declaration_expression
  type: (nullable_type type: [(identifier) (generic_name) (predefined_type)] @ref.receiver.proof.type)
  name: (identifier) @ref.receiver.proof)

; Every name a declaration declares, and the scopes an unstated one hides its
; name across.
(variable_declarator name: (identifier) @ref.receiver.hides)
(parameter name: (identifier) @ref.receiver.hides)
(implicit_parameter) @ref.receiver.hides
(foreach_statement left: (identifier) @ref.receiver.hides)
(catch_declaration name: (identifier) @ref.receiver.hides)
(declaration_pattern name: (identifier) @ref.receiver.hides)
(declaration_expression name: (identifier) @ref.receiver.hides)
(recursive_pattern name: (identifier) @ref.receiver.hides)
(list_pattern name: (identifier) @ref.receiver.hides)
(var_pattern name: (identifier) @ref.receiver.hides)
(tuple_pattern name: (identifier) @ref.receiver.hides)
(parenthesized_variable_designation name: (identifier) @ref.receiver.hides)
(from_clause name: (identifier) @ref.receiver.hides)
(let_clause . (identifier) @ref.receiver.hides)
(join_clause (identifier) @ref.receiver.hides . "in")
(query_expression "into" . (identifier) @ref.receiver.hides)
(join_into_clause (identifier) @ref.receiver.hides)
(_ parameters: (_)) @ref.receiver.hiding_scope
(property_declaration) @ref.receiver.hiding_scope
(field_declaration) @ref.receiver.hiding_scope
(event_field_declaration) @ref.receiver.hiding_scope
(global_statement) @ref.receiver.hiding_scope

; `this.x.List()`, and the `T?` fields and properties it reads.
(member_access_expression
  expression: (member_access_expression
    expression: "this"
    name: (identifier) @ref.receiver.field))
(variable_declaration
  type: (nullable_type type: (_) @ref.receiver.declared.type)
  (variable_declarator name: (identifier) @ref.receiver.declared))
(property_declaration
  type: (nullable_type type: (_) @ref.receiver.declared.type)
  name: (identifier) @ref.receiver.declared)

((member_access_expression expression: (identifier) @ref.receiver.unproven)
  (#eq? @ref.receiver.unproven "value"))

; `using` directives (S-518, CR-170, FR-RS-13). The four forms are told apart by
; the directive's leading keywords, read off its text, so exactly one pattern
; matches each directive and it records one row:
;
;   `using A.B;`             — a namespace wildcard: every type of `A.B` comes
;                              into view in this file (`@ref.import.asterisk`).
;                              Written inside a namespace block, its row records
;                              that block, read at the block's level (S-603).
;   `global using A.B;`      — the same wildcard for every file of the project
;                              (`@ref.import.global`), which the binder reads as
;                              every C# file under this file's directory: the
;                              `.csproj` is not read, and global usings sit at
;                              the project root by convention.
;   `using static A.B.C;`    — every static member of the type `A.B.C`
;                              (`@ref.import.static`, Java's `import static
;                              a.b.C.*`). A `global using static` is read as a
;                              plain one: this file only.
;   `using X = A.B.C;`       — an alias: one single-type row naming `A.B.C`,
;                              aliased by the name it binds, `X`
;                              (`@ref.import.alias`, S-520) — never by `C`, and
;                              `X` is not itself an import row.
;
; The namespace path is qualified or a single segment.
((using_directive
  !name
  [(qualified_name) (identifier)] @ref.import @ref.import.asterisk) @_using
  (#not-match? @_using "^(global\\s|using\\s+static\\s)"))

((using_directive
  !name
  [(qualified_name) (identifier)] @ref.import @ref.import.asterisk @ref.import.global) @_using
  (#match? @_using "^global\\s")
  (#not-match? @_using "^global\\s+using\\s+static\\s"))

((using_directive
  !name
  [(qualified_name) (identifier)] @ref.import @ref.import.static @ref.import.asterisk) @_using
  (#match? @_using "^(global\\s+)?using\\s+static\\s"))

(using_directive
  name: (identifier) @ref.import.alias
  [(qualified_name) (identifier)] @ref.import)

; Own-field access (`this.Count`): the binder proves an exactly-one Field
; candidate in the enclosing type for an `Accesses` edge (Method → Field); an
; ambiguous access stays unresolved (NFR-RA-05). `this` is an anonymous keyword
; token in tree-sitter-c-sharp, so it is matched as a string, not a node.
(member_access_expression
  expression: "this"
  name: (identifier) @ref.access)

; Bare field and property access (S-556, CR-177, FR-EX-22). `_count`, `Name` and
; `_log.Add(x)` read the enclosing class's own members with no `this.` written.
; Only the positions a VALUE is spelled in are listed — an operand, an argument,
; a receiver, a returned or assigned expression, an expression body, an
; interpolation, a condition, the source of a loop — so a name in a TYPE or
; DECLARATION position (`Status Status`, a member's name, a named argument's
; label) is never captured. The bare name of a CALL (`Check(x)`) is no value and
; is the method rows' concern. `collect_refs` records a row only where the
; method sits in a class that itself declares a field or property of that name
; (`bare_access`), and drops it where the same method declares a local of the
; name (`@ref.access.shadow` below); the binder then proves an exactly-one Field
; in that class (NFR-RA-05). A member the class inherits, or declares in another
; part of a `partial` type, is not seen: it stays unrecorded.
;
; Not captured: a constructor's or an accessor's body (neither is a node), the
; flat names of an anonymous object (`new { A, B = x }`) and of a `with`
; initializer, whose members and values share one list; a `nameof(Name)` and the
; member side of an object initializer (`new T { Name = Name }`) are dropped in
; `collect_refs`, which can see the call and the initializer.
[
  (argument !name (identifier) @ref.access.bare)
  (argument name: (identifier) . (identifier) @ref.access.bare)
  (member_access_expression expression: (identifier) @ref.access.bare)
  (conditional_access_expression condition: (identifier) @ref.access.bare)
  (element_access_expression expression: (identifier) @ref.access.bare)
  (assignment_expression left: (identifier) @ref.access.bare)
  (assignment_expression right: (identifier) @ref.access.bare)
  (binary_expression left: (identifier) @ref.access.bare)
  (binary_expression right: (identifier) @ref.access.bare)
  (conditional_expression condition: (identifier) @ref.access.bare)
  (conditional_expression consequence: (identifier) @ref.access.bare)
  (conditional_expression alternative: (identifier) @ref.access.bare)
  (prefix_unary_expression (identifier) @ref.access.bare)
  (postfix_unary_expression (identifier) @ref.access.bare)
  (parenthesized_expression (identifier) @ref.access.bare)
  (await_expression (identifier) @ref.access.bare)
  (cast_expression value: (identifier) @ref.access.bare)
  (as_expression left: (identifier) @ref.access.bare)
  (is_expression left: (identifier) @ref.access.bare)
  (is_pattern_expression expression: (identifier) @ref.access.bare)
  (range_expression (identifier) @ref.access.bare)
  (variable_declarator name: (_) (identifier) @ref.access.bare)
  (initializer_expression (identifier) @ref.access.bare)
  (lambda_expression body: (identifier) @ref.access.bare)
  (arrow_expression_clause (identifier) @ref.access.bare)
  (return_statement (identifier) @ref.access.bare)
  (yield_statement (identifier) @ref.access.bare)
  (throw_statement (identifier) @ref.access.bare)
  (throw_expression (identifier) @ref.access.bare)
  (interpolation (identifier) @ref.access.bare)
  (if_statement condition: (identifier) @ref.access.bare)
  (while_statement condition: (identifier) @ref.access.bare)
  (do_statement condition: (identifier) @ref.access.bare)
  (for_statement condition: (identifier) @ref.access.bare)
  (foreach_statement right: (identifier) @ref.access.bare)
  (switch_statement value: (identifier) @ref.access.bare)
  (switch_expression . (identifier) @ref.access.bare)
  (switch_expression_arm (identifier) @ref.access.bare)
  (when_clause (identifier) @ref.access.bare)
  (lock_statement (identifier) @ref.access.bare)
  (using_statement (identifier) @ref.access.bare)
]

; Names a method declares (S-556). A bare name the method declares is the
; local, not the class's member, so each declaring node marks its name and
; `collect_refs` drops the method's `@ref.access.bare` rows headed by it. The
; whole method is read as one scope, which can lose an access (a field read in
; a block beside an unrelated local of the same name) and never invents one.
; `var` patterns and `var (a, b)` deconstructions parse as `declaration_pattern`
; and `tuple_pattern`, so they need no pattern of their own; `x is var (a, _)`
; parses as a call on `x is var`, so its names are read as arguments (a stated
; ceiling of tree-sitter-c-sharp 0.23).
(parameter name: (identifier) @ref.access.shadow)
(implicit_parameter) @ref.access.shadow
(variable_declarator name: (identifier) @ref.access.shadow)
(foreach_statement left: (identifier) @ref.access.shadow)
(catch_declaration name: (identifier) @ref.access.shadow)
(declaration_pattern name: (identifier) @ref.access.shadow)
(recursive_pattern name: (identifier) @ref.access.shadow)
(list_pattern name: (identifier) @ref.access.shadow)
(declaration_expression name: (identifier) @ref.access.shadow)
(tuple_pattern name: (identifier) @ref.access.shadow)
(from_clause name: (identifier) @ref.access.shadow)
(let_clause . (identifier) @ref.access.shadow)
(join_clause . (identifier) @ref.access.shadow)
(join_into_clause (identifier) @ref.access.shadow)

;   @ref.extends — anchored to each declaration that owns a `base_list`. An
;   enum's (`enum E : byte`) names its underlying primitive, not a type. A
;   record's primary-constructor base (`record R(int X) : Base(X)`) is its own
;   node; a class's (`class P(int x) : Base(x)`) is a plain entry with arguments.
(class_declaration
  (base_list [(identifier) (qualified_name) (generic_name) (alias_qualified_name)] @ref.extends))
(struct_declaration
  (base_list [(identifier) (qualified_name) (generic_name) (alias_qualified_name)] @ref.extends))
(interface_declaration
  (base_list [(identifier) (qualified_name) (generic_name) (alias_qualified_name)] @ref.extends))
(record_declaration
  (base_list [(identifier) (qualified_name) (generic_name) (alias_qualified_name)] @ref.extends))
(record_declaration
  (base_list (primary_constructor_base_type type: (_) @ref.extends)))

;   @ref.extends.partial — a `partial` declaration names itself (S-557). Only a
;   class, struct, interface or record can be partial; a `partial` method is a
;   member, not a type part. `@ref.extends.partial.arity` is its type parameter
;   list, if any: `Box` and `Box<T>` are two types, so only parts of one arity
;   are one.
(class_declaration
  (modifier) @ref.extends.partial
  name: (identifier) @ref.extends
  (type_parameter_list)? @ref.extends.partial.arity
  (#eq? @ref.extends.partial "partial"))
(struct_declaration
  (modifier) @ref.extends.partial
  name: (identifier) @ref.extends
  (type_parameter_list)? @ref.extends.partial.arity
  (#eq? @ref.extends.partial "partial"))
(interface_declaration
  (modifier) @ref.extends.partial
  name: (identifier) @ref.extends
  (type_parameter_list)? @ref.extends.partial.arity
  (#eq? @ref.extends.partial "partial"))
(record_declaration
  (modifier) @ref.extends.partial
  name: (identifier) @ref.extends
  (type_parameter_list)? @ref.extends.partial.arity
  (#eq? @ref.extends.partial "partial"))

; ── Argument count (S-591, CR-190, FR-EX-32) ─────────────────────────────────
; The `@arity.*` vocabulary the extraction engine reads (`extract::arity`): every
; argument list, whose named children a call row counts, and the forms that
; make a count unknown. Captures record no row of their own, so every ledger
; target is unchanged.
(argument_list) @arity.arguments
