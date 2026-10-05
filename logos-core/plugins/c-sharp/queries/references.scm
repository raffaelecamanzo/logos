; C# reference-extraction query (S-057, CR-009, capability = "references").
;
;   @ref.method — a method invocation's name (`service.List()`, `List()`); a
;                 Method-form row that binds by its receiver's SHAPE, recorded
;                 by the `@ref.receiver.*` markers below (S-514, S-516, CR-169).
;   @ref.import — a `using` directive's namespace path (`Microsoft.AspNetCore.Mvc`);
;                 canonicalised (dots → `::`) into the ledger form feeding the
;                 binder and the framework candidacy gate (FR-FW-04). A plain
;                 `using N;` brings every type of `N` into view, so it is marked
;                 a wildcard (`@ref.import.asterisk`, S-518, FR-RS-13); see the
;                 `using` section below for `global`, `static` and aliases.
;   @ref.access — an own-field access (`this.X`): a method reading a field of its
;                 own type (CR-005, FR-EX-08); the bound LCOM4 input.
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
;
; Receiver-shape markers (S-516, FR-EX-13, FR-RS-12) — they record no row of
; their own; each names the receiver of the `@ref.method` that shares its parent
; node (the `member_access_expression`, or for a bare call the invocation):
;
;   @ref.receiver.self     — `this.List()`: binds among the enclosing class's
;                            own members only.
;   @ref.receiver.super    — `base.List()`: binds only through a proven
;                            `Extends` of the caller's type — its base class,
;                            never an interface (`@ref.extends` below).
;   @ref.receiver.other    — every other receiver (`service.List()`,
;                            `Enumerable.Range()`): never bound through the
;                            caller's scope (`no-receiver-evidence`).
;   @ref.receiver.implicit — `List()`, no receiver: under this plugin's
;                            `implicit_receiver = "self"` a call on the current
;                            instance inside a type, a free call outside one.
;
; Droppable on disk at `.logos/plugins/c-sharp/queries/references.scm`.
;
; Deliberately NOT captured in v1: `new T()` construction references (no
; constructor nodes exist to bind them to — see symbols.scm), `using static`
; member binding, generic type arguments, and the generic (`M<T>()`) and
; conditional (`x?.M()`) invocation forms.

; Bare call (`List()`): no receiver written.
(invocation_expression
  function: (identifier) @ref.method @ref.receiver.implicit)

; Member call (`service.List()`, `this.List()`, `base.List()`): the method name.
(invocation_expression
  function: (member_access_expression
    name: (identifier) @ref.method))

; The receiver of a member call. `this` and `base` are anonymous keyword tokens
; in tree-sitter-c-sharp, matched as strings; `(_)` matches named nodes only,
; so the catch-all never reaches either keyword.
(member_access_expression
  expression: "this" @ref.receiver.self)

(member_access_expression
  expression: "base" @ref.receiver.super)

(member_access_expression
  expression: (_) @ref.receiver.other)

; `using` directives (S-518, CR-170, FR-RS-13). The four forms are told apart by
; the directive's leading keywords, read off its text, so exactly one pattern
; matches each directive and it records one row:
;
;   `using A.B;`             — a namespace wildcard: every type of `A.B` comes
;                              into view in this file (`@ref.import.asterisk`).
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

;   @ref.extends — anchored to each declaration that owns a `base_list`. An
;   enum's (`enum E : byte`) names its underlying primitive, not a type, and a
;   record's primary-constructor base (`record R(int X) : Base(X)`) is its type.
(class_declaration
  (base_list [(identifier) (qualified_name) (generic_name) (alias_qualified_name)] @ref.extends))
(class_declaration
  (base_list (primary_constructor_base_type type: (_) @ref.extends)))
(struct_declaration
  (base_list [(identifier) (qualified_name) (generic_name) (alias_qualified_name)] @ref.extends))
(interface_declaration
  (base_list [(identifier) (qualified_name) (generic_name) (alias_qualified_name)] @ref.extends))
(record_declaration
  (base_list [(identifier) (qualified_name) (generic_name) (alias_qualified_name)] @ref.extends))
(record_declaration
  (base_list (primary_constructor_base_type type: (_) @ref.extends)))
