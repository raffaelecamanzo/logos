; PHP reference-extraction query (S-060, capability = "references").
;
;   @ref.call   — a free function call's name (`callee()`); bound by scope
;                 under every policy (S-551, FR-RS-26): the file's own
;                 function, its namespace's, then — from a namespaced file —
;                 the global one; from a file declaring no namespace, the
;                 exactly-one function another such file declares. A name
;                 declared twice binds neither. And a static call on a named class
;                 (`Foo::bar()`), recorded with its class (`@ref.call.scope`)
;                 as the Path `Foo::bar` (S-547, FR-RS-25), bound among `Foo`'s
;                 methods.
;   @ref.method — an instance (`$svc->list()`) or static (`self::bar()`,
;                 `$cls::bar()`) method call name on any other receiver;
;                 name-only, bound by its receiver's SHAPE (the markers below)
;                 — or, where the file proves the receiver's type `T`, the
;                 Path `T::list` a static call records (S-552, below).
;   @ref.instantiates — the class of `new Foo(…)` (S-547): `Instantiates`.
;   @ref.import — a `use` clause's namespace path (`Illuminate\Support\…\Route`);
;                 canonicalised (backslash → `::`) into the ledger form feeding
;                 the binder and the framework candidacy gate (FR-FW-04). And
;                 a literal `require`/`include` path (S-551), marked
;                 `@ref.import.file`: an `Imports` row to the file it names.
;   @ref.import.alias — the local name a `use A\B as C;` binds (`C`; S-520). The
;                 row's alias is that name, not `B`; without an `as` it stays
;                 the path's last segment.
;   @ref.access — an own-property access (`$this->balance`): a method reading a
;                 field of its own class (CR-005, FR-EX-08), the bound LCOM4 input.
;   @ref.extends — a class's parent (`extends Base`), an interface's parents:
;                 `Extends` (class → class, interface → interface; S-522,
;                 FR-RS-15).
;   @ref.implements — a class's or enum's interfaces (`implements I`), and the
;                 traits a class, trait or enum `use`s (`use Loggable;`):
;                 `Implements` (→ interface or trait). Both bind through the
;                 declared namespace's rungs — the file's `use` imports, its own
;                 namespace, then the fully-qualified name. A name written with
;                 a leading `\` is fully qualified, read from the global
;                 namespace alone. One written `namespace\X` is not captured:
;                 its current-namespace prefix is no name the imports read.
;   @ref.implements.trait — a MARKER on a trait `use`: the row records that
;                 the class uses the trait. A used trait's method outranks
;                 every inherited one, so the class's hierarchy ends there for
;                 a `$this->m()` / `parent::m()` walk (S-522).
;
; Droppable on disk at `.logos/plugins/php/queries/references.scm`.
;
; Deliberately NOT captured: dynamic calls (`$obj->$method()`,
; `call_user_func`), variable-variable indirection, a class held in a variable
; (`$cls::m()`, `new $cls`) — they stay honestly unresolved (NFR-RA-05, never
; fabricate).

(function_call_expression
  function: (name) @ref.call)

(member_call_expression
  name: (name) @ref.method)

; A static call on a named class (`Foo::bar()`, `\App\Foo::bar()`) records the
; Path `Foo::bar` — its class (`@ref.call.scope`, S-547) and the method — which
; the binder reads `Foo` through the file's `use` imports and namespace and binds
; among `Foo`'s methods (FR-RS-25). A qualified name is captured only when it is
; written fully qualified (a leading `\`): `Sub\Foo` is read by PHP under the
; file's own namespace (`App\Sub\Foo`), a prefix the binder has no rung for in a
; language with no enclosing namespaces, and its last rung would read it as the
; global `Sub\Foo` — another class (NFR-RA-05, never fabricate). Every other scope
; keeps the bare method name and its receiver's SHAPE: `self::` / `static::` /
; `parent::` (a `relative_scope`) bind per the markers below, and a relative
; qualified name (`Sub\Foo::m()`), `namespace\Foo::m()`, a class held in a variable
; (`$cls::m()`), a call result or any other expression is `other` and binds
; nothing.
(scoped_call_expression
  scope: (name) @ref.call.scope
  name: (name) @ref.call)

(scoped_call_expression
  scope: (qualified_name) @ref.call.scope
  name: (name) @ref.call
  (#match? @ref.call.scope "^\\\\"))

(scoped_call_expression
  scope: (qualified_name) @_scope
  name: (name) @ref.method
  (#not-match? @_scope "^\\\\"))

; The scope kinds besides a name and a qualified name (the complement of the
; patterns above over the grammar's `scope` field types — re-check it against
; the grammar's `node-types.json` on a grammar bump).
(scoped_call_expression
  scope: [
    (relative_scope) (relative_name) (variable_name) (dynamic_variable_name)
    (member_access_expression) (nullsafe_member_access_expression)
    (member_call_expression) (nullsafe_member_call_expression)
    (function_call_expression) (scoped_call_expression)
    (scoped_property_access_expression) (class_constant_access_expression)
    (subscript_expression) (parenthesized_expression) (object_creation_expression)
    (string) (encapsed_string) (heredoc) (nowdoc) (array_creation_expression)
    (cast_expression)
  ]
  name: (name) @ref.method)

;   @ref.instantiates — the class of `new Foo(…)` (`new \App\Foo`): an
;   `Instantiates` row bound among the classes `Foo` names (S-547, FR-RS-25). A
;   qualified name is captured only when fully qualified, as above. `new self` /
;   `new static` / `new parent`, `new $cls` and `new class { … }` name no class
;   and record nothing.
(object_creation_expression
  (name) @ref.instantiates
  (#not-match? @ref.instantiates "^(?i:self|static|parent)$"))
(object_creation_expression
  (qualified_name) @ref.instantiates
  (#match? @ref.instantiates "^\\\\"))

;   Receiver markers (S-515, CR-169, FR-EX-13) — MARKERS, recording nothing on
;   their own. Each names the receiver (`object`) or scope (`scope`) of the
;   method call above: its captured node shares the call expression the
;   `@ref.method` name sits in, and the call records that SHAPE, which the
;   binder dispatches on (FR-RS-12).
;
;   @ref.receiver.self      — `$this->m()`, `self::m()`, `static::m()`: binds
;                             among the enclosing class's own members only.
;   @ref.receiver.super     — `parent::m()`: binds only through a proven
;                             `Extends` of the caller's class (`@ref.extends`
;                             below) — the nearest parent holding exactly one
;                             `m` — and never reaches the caller's own class.
;   @ref.receiver.other     — every receiver or scope (`$svc->m()`, `$cls::m()`,
;                             `$this->repo->m()`): never binds, unless the
;                             typing markers below prove its type. `self`/`super`
;                             outrank it. A named class's (`Foo::m()`) is marked
;                             too, but its row is the Path above, which carries
;                             no shape.
;   @ref.receiver.anonymous — an anonymous class's body (`new class { … }`): it
;                             has no class node, so a `$this->m()` inside is
;                             never the enclosing class's `m` — `other`, unless
;                             the body declares `m` itself.
;
;   An unqualified call (`m()`) is never a call on the instance in PHP: it is
;   the free `@ref.call` above (`implicit_receiver` stays "none").
((member_call_expression object: (variable_name) @ref.receiver.self)
  (#eq? @ref.receiver.self "$this"))
((scoped_call_expression scope: (relative_scope) @ref.receiver.self)
  (#any-of? @ref.receiver.self "self" "static"))
((scoped_call_expression scope: (relative_scope) @ref.receiver.super)
  (#eq? @ref.receiver.super "parent"))
(member_call_expression object: (_) @ref.receiver.other)
(scoped_call_expression scope: (_) @ref.receiver.other)
(anonymous_class body: (declaration_list) @ref.receiver.anonymous)

;   Receiver typing (S-552, FR-RS-27) — MARKERS. Where the file proves an
;   `other` receiver's type `T`, the call is recorded as the Path `T::m` a
;   static call `T::m()` records (`@ref.call.scope` above), read through the
;   file's `use` imports and namespace and bound among `T`'s methods; anything
;   unproven keeps its `other` row (`no-receiver-evidence`). `T` is one
;   unqualified name: a nullable `?T` proves `T`; `self`, `static`, `parent`, a
;   union, intersection, primitive or qualified type (`\App\T`, `Sub\T`)
;   proves nothing.
;
;   A variable, `$m->m()`, speaks Rust's binding vocabulary (S-587): it is
;   typed by the ONE binding of `$m` in scope at the call, when that binding
;   carries a proof. Two bindings in scope — a re-assignment, a closure's own
;   parameter beside the method's — prove nothing, nor does one with no proof.
;
;   @ref.receiver.variable — `$m->m()`: the receiver, read as its name `m`.
;   @ref.receiver.binding  — every name a callable's parameter binds — a
;                            plain, variadic or promoted one (`.scope`: that
;                            function, method, closure or arrow function) —
;                            and every other name the code binds, in scope in
;                            the whole outermost callable (no companion: PHP
;                            scopes a variable to its function, never to a
;                            block): an assignment's (`=`, `=&`, `??=`, …, a
;                            `list()` / `[…]` destructuring), a `foreach`
;                            key or value, a `catch` variable, a `global` or
;                            `static` one. A closure's `use ($m)` binds no
;                            new name: it is the enclosing binding's value.
;   @ref.receiver.proof    — a binding's proven type: a typed parameter or
;                            promoted property (`.type`: `T`, `?T`), `$m = new
;                            T(…)` (`.literal`: `T`).
;
;   `$this->svc->m()` speaks Java's (S-467): the property `svc` the enclosing
;   class declares itself — a property declaration or a promoted constructor
;   parameter, every one of which this query states:
;
;   @ref.receiver.field    — `$this->svc->m()`.
;   @ref.receiver.declared — a property, by its name without the `$`: its
;                            `.type` companion names a proven `T` (`T`, `?T`);
;                            its `.untyped` companion states that nothing
;                            else does — no type, a union, a primitive or a
;                            qualified one — and refuses the name. Of the
;                            two, `.type` wins. Every promoted parameter is a
;                            property of its class besides (`.field`, stated
;                            on the pattern that matches them all).
;
;   Stated ceilings: a property the class inherits is not its own; a
;   nullsafe call (`$m?->m()`, captured by no `@ref.method`), a chained or
;   a dynamic receiver, a variable an out-parameter (`preg_match(…, $m)`),
;   `extract()`, `$$name` or an included file assigns, and a static or
;   `self`-typed property keep their row. A parameter is in scope across its
;   whole callable, so a closure without `use ($m)`, a nested function or an
;   anonymous class's method reading the outer `$m` — undefined there in PHP —
;   is typed by the outer binding.
((member_call_expression object: (variable_name) @ref.receiver.variable)
  (#not-eq? @ref.receiver.variable "$this"))

(_
  parameters: (formal_parameters
    [(simple_parameter name: (_) @ref.receiver.binding)
     (variadic_parameter name: (_) @ref.receiver.binding)
     (property_promotion_parameter name: (_) @ref.receiver.binding)])) @ref.receiver.binding.scope
(assignment_expression left: [(variable_name) (list_literal)] @ref.receiver.binding)
(reference_assignment_expression left: [(variable_name) (list_literal)] @ref.receiver.binding)
(augmented_assignment_expression left: (variable_name) @ref.receiver.binding)
(foreach_statement . (_) . [(variable_name) (pair) (by_ref) (list_literal)] @ref.receiver.binding)
(catch_clause name: (variable_name) @ref.receiver.binding)
(global_declaration (variable_name) @ref.receiver.binding)
(static_variable_declaration name: (variable_name) @ref.receiver.binding)

((simple_parameter
  type: [(named_type (name) @ref.receiver.proof.type)
         (optional_type (named_type (name) @ref.receiver.proof.type))]
  name: (variable_name (name) @ref.receiver.proof))
  (#not-match? @ref.receiver.proof.type "^(?i:self|static|parent)$"))
((property_promotion_parameter
  type: [(named_type (name) @ref.receiver.proof.type)
         (optional_type (named_type (name) @ref.receiver.proof.type))]
  name: [(variable_name (name) @ref.receiver.proof)
         (by_ref (variable_name (name) @ref.receiver.proof))])
  (#not-match? @ref.receiver.proof.type "^(?i:self|static|parent)$"))
((assignment_expression
  left: (variable_name (name) @ref.receiver.proof)
  right: (object_creation_expression (name) @ref.receiver.proof.literal))
  (#not-match? @ref.receiver.proof.literal "^(?i:self|static|parent)$"))

((member_call_expression
  object: (member_access_expression
    object: (variable_name (name) @_self)
    name: (name) @ref.receiver.field))
  (#eq? @_self "this"))

((property_declaration
  type: [(named_type (name) @ref.receiver.declared.type)
         (optional_type (named_type (name) @ref.receiver.declared.type))]
  (property_element name: (variable_name (name) @ref.receiver.declared)))
  (#not-match? @ref.receiver.declared.type "^(?i:self|static|parent)$"))
(property_declaration
  (property_element name: (variable_name (name) @ref.receiver.declared))) @ref.receiver.declared.untyped
((property_promotion_parameter
  type: [(named_type (name) @ref.receiver.declared.type)
         (optional_type (named_type (name) @ref.receiver.declared.type))]
  name: [(variable_name (name) @ref.receiver.declared)
         (by_ref (variable_name (name) @ref.receiver.declared))])
  (#not-match? @ref.receiver.declared.type "^(?i:self|static|parent)$"))
(property_promotion_parameter
  name: [(variable_name (name) @ref.receiver.declared)
         (by_ref (variable_name (name) @ref.receiver.declared))]) @ref.receiver.declared.field @ref.receiver.declared.untyped

;   A qualified `use` clause, and each clause of a group (`use
;   App\Mail\{Mailer, Pager as P};`), recorded under the group's prefix
;   (`@ref.import.from`) — so a type a typed receiver names (S-552) is read
;   through the import that brought it in, never the caller's own namespace.
;   The clause's first name is the import; an `as` name is its alias. Stated
;   ceilings: a one-segment `use Foo;` (a global class) records no import, so
;   `Foo` is read in the caller's namespace before the global one — another
;   class only where that namespace declares a `Foo` too; and a file declaring
;   two namespaces reads every `use` as the whole file's.
(namespace_use_declaration
  (namespace_use_clause
    (qualified_name) @ref.import
    alias: (name)? @ref.import.alias))
(namespace_use_declaration
  (namespace_name) @ref.import.from
  body: (namespace_use_group
    (namespace_use_clause
      .
      [(name) (qualified_name)] @ref.import
      alias: (name)? @ref.import.alias)))

;   @ref.import + @ref.import.file — a file include (S-551, FR-RS-26):
;   `require`, `require_once`, `include` and `include_once` of a string
;   literal, bare or parenthesised (`require_once( 'core.php' );`). The
;   `@ref.import.file` MARKER says the text is a file path, never a `\` member
;   path: the row names the file whole, extension kept, read against the
;   including file's directory, and binds to that file. One appended to
;   `__DIR__` (`__DIR__ . '/lib/a.php'`) is read the same way; its
;   `@ref.import.file.dir` MARKER says the path follows the directory. Any
;   other argument — a variable, any other concatenation (`dirname(__FILE__) .
;   '…'`, `$t_dir . '…'`), a string interpolating a variable — names no file
;   the tree states, and records nothing (NFR-RA-05). So does mantisbt's
;   `require_api( '…' )`, a function call.
([
  (include_expression [(string) (encapsed_string)] @ref.import)
  (include_once_expression [(string) (encapsed_string)] @ref.import)
  (require_expression [(string) (encapsed_string)] @ref.import)
  (require_once_expression [(string) (encapsed_string)] @ref.import)
  (include_expression (parenthesized_expression [(string) (encapsed_string)] @ref.import))
  (include_once_expression (parenthesized_expression [(string) (encapsed_string)] @ref.import))
  (require_expression (parenthesized_expression [(string) (encapsed_string)] @ref.import))
  (require_once_expression (parenthesized_expression [(string) (encapsed_string)] @ref.import))
] @ref.import.file
  (#not-match? @ref.import "[$]"))

([
  (include_expression (binary_expression left: (name) @ref.import.file.dir operator: "." right: [(string) (encapsed_string)] @ref.import))
  (include_once_expression (binary_expression left: (name) @ref.import.file.dir operator: "." right: [(string) (encapsed_string)] @ref.import))
  (require_expression (binary_expression left: (name) @ref.import.file.dir operator: "." right: [(string) (encapsed_string)] @ref.import))
  (require_once_expression (binary_expression left: (name) @ref.import.file.dir operator: "." right: [(string) (encapsed_string)] @ref.import))
  (include_expression (parenthesized_expression (binary_expression left: (name) @ref.import.file.dir operator: "." right: [(string) (encapsed_string)] @ref.import)))
  (include_once_expression (parenthesized_expression (binary_expression left: (name) @ref.import.file.dir operator: "." right: [(string) (encapsed_string)] @ref.import)))
  (require_expression (parenthesized_expression (binary_expression left: (name) @ref.import.file.dir operator: "." right: [(string) (encapsed_string)] @ref.import)))
  (require_once_expression (parenthesized_expression (binary_expression left: (name) @ref.import.file.dir operator: "." right: [(string) (encapsed_string)] @ref.import)))
] @ref.import.file
  (#match? @ref.import.file.dir "^(?i:__DIR__)$")
  (#not-match? @ref.import "[$]"))

;   @ref.access — `$this->field`: an own-field read/write. `member_access_expression`
; is a distinct node from `member_call_expression`, so this never double-captures
; a method call. The class lexically Contains both the method and its
; `property_declaration` fields, so resolution binds an exactly-one Field
; candidate to an `Accesses` edge (Method → Field); an ambiguous access stays
; unresolved (NFR-RA-05).
((member_access_expression
  object: (variable_name (name) @_recv)
  name: (name) @ref.access)
  (#eq? @_recv "this"))

;   @ref.extends / @ref.implements — anchored to the declaration that owns the
;   clause, so an anonymous class's (`new class extends Base {}`) is never read
;   as the enclosing function's: it has no class node to relate.
(class_declaration
  (base_clause [(name) (qualified_name)] @ref.extends))
(interface_declaration
  (base_clause [(name) (qualified_name)] @ref.extends))
(class_declaration
  (class_interface_clause [(name) (qualified_name)] @ref.implements))
(enum_declaration
  (class_interface_clause [(name) (qualified_name)] @ref.implements))
(class_declaration
  body: (declaration_list
    (use_declaration [(name) (qualified_name)] @ref.implements) @ref.implements.trait))
(trait_declaration
  body: (declaration_list
    (use_declaration [(name) (qualified_name)] @ref.implements) @ref.implements.trait))
(enum_declaration
  body: (enum_declaration_list
    (use_declaration [(name) (qualified_name)] @ref.implements) @ref.implements.trait))

; ── Argument count (S-591, CR-190, FR-EX-32) ─────────────────────────────────
; The `@arity.*` vocabulary the extraction engine reads (`extract::arity`): every
; argument list, whose named children a call row counts, and the forms that
; make a count unknown. Captures record no row of their own, so every ledger
; target is unchanged.
; `...$xs` spreads; a call's `object`/`scope` is its receiver, not its callee.
(arguments) @arity.arguments
(arguments (argument (variadic_unpacking)) @arity.spread)
; A first-class callable `f(...)` makes a closure and passes no argument: its
; count says nothing about `f`'s parameters (S-592).
(arguments (variadic_placeholder) @arity.spread)
(member_call_expression object: (_) @arity.receiver)
(nullsafe_member_call_expression object: (_) @arity.receiver)
(scoped_call_expression scope: (_) @arity.receiver)
