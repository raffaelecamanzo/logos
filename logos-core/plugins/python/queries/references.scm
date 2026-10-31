; Python reference-extraction query (S-015, capability = "references").
;
; The capture name carries the reference shape (see extract::collect_refs):
;   @ref.call   — a plain-identifier call (`f()`); the text is the path.
;   @ref.method — an attribute call (`obj.m()`, `module.f()`); it binds by
;                 its receiver's SHAPE (the markers below), or by its TYPE where
;                 the file proves one (`svc.m()` after `svc: Service`, recorded
;                 `Service::m`; S-546, FR-RS-24) — unless
;                 its receiver is a name an import binds (`curl.post()` after
;                 `from hc.lib import curl`): that call is recorded through
;                 the import's path, `hc::lib::curl::post` (S-544, FR-RS-23).
;                 A bare decorator is that call too (S-545, FR-RS-23): `@name` is
;                 `@ref.call` and `@mod.name` is `@ref.method` — each the
;                 decorator applied to the function, recorded where a call would
;                 be, so it binds as the same text in call position does
;                 (`@authorize` after `from hc.api.decorators import authorize`
;                 binds the decorator). `@property`, `@staticmethod` and the
;                 other builtins name no workspace declaration, so they stay
;                 unbound — unless the aggressive policy's unique-name guess
;                 finds a workspace function of that name, as it does for the
;                 same call. A decorator written as a call (`@app.get("/")`) is
;                 the `call` pattern above and is never also a bare one; the
;                 receiver of `@mod.name` is always `other` (never `self`,
;                 never `super`), so only an imported module binds it.
;   @ref.import — an import path; the captured node's *text* is canonicalised
;                 (dots → `::`) into the ledger form that feeds both the
;                 binder and the framework candidacy gate (FR-FW-04).
;   @ref.import.from — a MARKER, recording nothing on its own: the module of a
;                 `from m import a, b` (S-519, FR-RS-14). Each imported name
;                 is its own `@ref.import` match, recorded as its path under
;                 that module (`m::a`, `m::b`) — one row per imported name. A
;                 relative module keeps its level as leading `.`/`..` segments
;                 (`from .rules import Rule` → `.::rules::Rule`, `from ..x
;                 import y` → `..::x::y`), which the binder reads from the
;                 importing file's package. With `@ref.import.asterisk`
;                 (`from m import *`) the row is a glob of the module itself.
;   @ref.import.alias — the local name an `as` import binds (`import a as b`,
;                 `from m import a as b` → `b`; S-520). The row's alias is that
;                 name, never the imported one, so a call to `a` does not bind
;                 through an import that renamed it (S-519 review) and `b()`
;                 does. The path is still recorded under the imported module.
;   @ref.extends — a class's base (`class A(B)`, `class A(models.Model)`,
;                 `class A(Base[T])` → `Base`): `Extends`, bound through the
;                 path modules to the one in-repository class it names (S-522,
;                 FR-RS-15). A keyword argument (`metaclass=…`) and a `*bases`
;                 splat are not bases.
;
; Droppable on disk at `.logos/plugins/python/queries/references.scm`
; (FR-PL-04, FR-PL-05).

(call
  function: (identifier) @ref.call)

(call
  function: (attribute
    attribute: (identifier) @ref.method))

(decorator
  (identifier) @ref.call)

(decorator
  (attribute
    attribute: (identifier) @ref.method))

;   Receiver markers (S-515, CR-169, FR-EX-13) — MARKERS, recording nothing on
;   their own. Each names the receiver of the attribute call above: its
;   captured node shares the `attribute` the `@ref.method` name sits in, and the
;   call records that SHAPE, which the binder dispatches on (FR-RS-12).
;
;   @ref.receiver.self  — `self.m()`, `cls.m()`: the conventional instance and
;                         class parameters; binds among the enclosing class's
;                         own members only.
;   @ref.receiver.super — `super().m()`: the inner `super` call is the
;                         receiver. Binds only through a proven `Extends` of
;                         the caller's class (`@ref.extends` below) — the
;                         nearest base holding exactly one `m` — and never
;                         reaches the caller's class. `super(A, self).m()`
;                         starts above a class it names, which the caller's
;                         own hierarchy does not decide: it is `other`.
;   @ref.receiver.other — every other receiver (`obj.m()`, `self.x.m()`,
;                         and `module.f()` when no import names `module`, or
;                         a local rebinds it): never binds through the
;                         caller's scope — typed where the file proves the
;                         receiver's type ("Receiver typing" below).
;                         `self`/`super` outrank it.
;
;   An unqualified call (`m()`) is never a call on the instance in Python: it
;   is the free `@ref.call` above (`implicit_receiver` stays "none").
((call
  function: (attribute object: (identifier) @ref.receiver.self))
  (#any-of? @ref.receiver.self "self" "cls"))
((call
  function: (attribute
    object: (call
      function: (identifier) @_super
      arguments: (argument_list) @_arguments) @ref.receiver.super))
  (#eq? @_super "super")
  (#eq? @_arguments "()"))
(call
  function: (attribute object: (_) @ref.receiver.other))
(decorator
  (attribute object: (_) @ref.receiver.other))

;   Receiver typing (S-546, CR-175, FR-RS-24, FR-RS-10) — markers recording no
;   row of their own (`extract::receiver`). Where the file proves an attribute
;   call's receiver type `T`, its Method row is retyped to the Path-form
;   `T::m`, keeping its `other` shape — `T` as the file writes it, which the
;   binder reads through the file's imports as it reads a written qualifier
;   (S-544) — and bound among `T`'s methods, or at the nearest level of its
;   proven `Extends` chain holding one. Anything unproven keeps its `other`
;   row (`no-receiver-evidence`).
;
;   A type is proven by an annotation naming ONE type, which the query reads
;   down to that type's name: `T`; `G[A]` (a `G`); `Optional[T]`,
;   `typing.Optional[T]`, `T | None` and `None | T` (a `T`); a string `"T"` or
;   `Optional["T"]` (a `T`). Or by a construction `x = T(…)`, `T` a capitalised
;   name (PEP 8's class names): `x = make()` is a function's result. A dotted
;   type (`models.T`, `"m.T"`), `Union[…]` or any other union, and `Annotated`,
;   `ClassVar` and `Final` prove nothing; a capitalised callee the binder finds
;   is a function proves nothing either. Each annotation alternative below
;   carries the predicate of its own branch: a capture a match did not take
;   passes its predicate (tree-sitter tests the nodes a match captured).
;
;   A plain name, `svc.m()`, speaks Rust's binding vocabulary (S-587): it is
;   typed by the ONE binding of `svc` in scope at the call, when that binding
;   carries a proof. Python binds a name for the whole of its function, so a
;   second binding anywhere in that function — a reassignment, an augmented
;   assignment, a `for`, `with`, `except` or `case … as` target, a walrus, a
;   nested `def` or `class`, an import, a `case` pattern, a comprehension or
;   lambda variable, an inner function's parameter — proves nothing; nor does a
;   module-level binding read inside a function. A sibling function's local and
;   a class body's name are not in scope in a function at all.
;
;   @ref.receiver.variable — `svc.m()`: the receiver name (never `self`/`cls`).
;   @ref.receiver.binding  — every name a statement binds: a parameter
;                            (`.scope`: its function's or lambda's body, never
;                            its defaults, which the enclosing scope evaluates);
;                            every other form (`.local`: the innermost function
;                            around the statement and the functions nested in
;                            it — at module level, the calls outside every
;                            function); and a class body's names — its
;                            assignments, `def`s and nested classes (`.class`:
;                            that body alone, which no method sees).
;   @ref.receiver.proof    — a binding's proven type: an annotated parameter or
;                            name (`.type`), `x = T(…)` (`.literal`).
;   @ref.receiver.unproven — a `global` or `nonlocal` name: another function may
;                            rebind it, so it proves nothing anywhere in the
;                            file.
;
;   `self.x.m()` speaks Java's `field` (S-467): the attribute `x` the enclosing
;   class declares itself (S-543's Fields), typed by the class's own
;   declarations of it, every one of which this query states:
;
;   @ref.receiver.field    — `self.x.m()`.
;   @ref.receiver.declared — an attribute: an assignment or annotation of the
;                            class body, or a method's `self.x` write (`.field`:
;                            an attribute of its class wherever it sits, stated
;                            with every write's `.untyped`). Its
;                            `.type` companion names a proven `T` (an annotation,
;                            or `T(…)` assigned); its `.untyped` companion states
;                            every write, and refuses the name unless a `.type`
;                            names the same node — so one untyped write, or two
;                            disagreeing ones, prove nothing — and every `def`
;                            (a `@property` included) and nested class of the
;                            class body, which the name then calls instead.
;
;   Stated ceilings: a `cls.x` or inherited attribute, a call on a class name
;   (`Service.m()`, S-544's), a chained or subscripted receiver, a module-level
;   binding read in a function, and a class-body assignment under a compound
;   statement (S-543 declares no Field for it) keep their row.
((call
  function: (attribute object: (identifier) @ref.receiver.variable))
  (#not-any-of? @ref.receiver.variable "self" "cls"))

(function_definition
  parameters: (parameters
    [(identifier) @ref.receiver.binding
     (typed_parameter (identifier) @ref.receiver.binding)
     (default_parameter name: (_) @ref.receiver.binding)
     (typed_default_parameter name: (_) @ref.receiver.binding)
     (list_splat_pattern (identifier) @ref.receiver.binding)
     (dictionary_splat_pattern (identifier) @ref.receiver.binding)
     (typed_parameter [(list_splat_pattern) (dictionary_splat_pattern)] @ref.receiver.binding)])
  body: (block) @ref.receiver.binding.scope)
(lambda
  parameters: (lambda_parameters
    [(identifier) @ref.receiver.binding
     (default_parameter name: (_) @ref.receiver.binding)
     (list_splat_pattern (identifier) @ref.receiver.binding)
     (dictionary_splat_pattern (identifier) @ref.receiver.binding)])
  body: (_) @ref.receiver.binding.scope)
(assignment
  left: [(identifier) (pattern_list) (tuple_pattern) (list_pattern)] @ref.receiver.binding) @ref.receiver.binding.local
(augmented_assignment left: (identifier) @ref.receiver.binding) @ref.receiver.binding.local
(for_statement left: (_) @ref.receiver.binding) @ref.receiver.binding.local
(for_in_clause left: (_) @ref.receiver.binding) @ref.receiver.binding.local
(as_pattern alias: (as_pattern_target) @ref.receiver.binding) @ref.receiver.binding.local
(named_expression name: (identifier) @ref.receiver.binding) @ref.receiver.binding.local
(case_pattern) @ref.receiver.binding @ref.receiver.binding.local
(class_definition name: (identifier) @ref.receiver.binding) @ref.receiver.binding.local
(function_definition name: (identifier) @ref.receiver.binding) @ref.receiver.binding.local
(import_statement name: (dotted_name . (identifier) @ref.receiver.binding)) @ref.receiver.binding.local
(import_statement name: (aliased_import alias: (identifier) @ref.receiver.binding)) @ref.receiver.binding.local
(import_from_statement name: (dotted_name (identifier) @ref.receiver.binding)) @ref.receiver.binding.local
(import_from_statement name: (aliased_import alias: (identifier) @ref.receiver.binding)) @ref.receiver.binding.local
(global_statement (identifier) @ref.receiver.unproven)
(nonlocal_statement (identifier) @ref.receiver.unproven)
(class_definition
  body: (block
    [(expression_statement
       (assignment
         left: [(identifier) @ref.receiver.binding
                (pattern_list (identifier) @ref.receiver.binding)
                (tuple_pattern (identifier) @ref.receiver.binding)]))
     (function_definition name: (identifier) @ref.receiver.binding)
     (decorated_definition definition: (function_definition name: (identifier) @ref.receiver.binding))
     (class_definition name: (identifier) @ref.receiver.binding)])) @ref.receiver.binding.class

((typed_parameter
  (identifier) @ref.receiver.proof
  type: (type
    [(identifier) @ref.receiver.proof.type
     (generic_type (identifier) @ref.receiver.proof.type (type_parameter))
     (generic_type
       (identifier) @_optional
       (type_parameter . (type [(identifier) @ref.receiver.proof.type (string (string_content) @ref.receiver.proof.type)]) .))
     (subscript
       value: (attribute) @_typing_optional
       subscript: [(identifier) @ref.receiver.proof.type (string (string_content) @ref.receiver.proof.type)])
     (binary_operator left: (identifier) @ref.receiver.proof.type operator: "|" right: (none))
     (binary_operator left: (none) operator: "|" right: (identifier) @ref.receiver.proof.type)
     (string (string_content) @ref.receiver.proof.type)]))
  (#not-any-of? @ref.receiver.proof.type "Optional" "Union" "Annotated" "ClassVar" "Final")
  (#match? @ref.receiver.proof.type "^[A-Za-z_][A-Za-z0-9_]*$")
  (#eq? @_optional "Optional")
  (#eq? @_typing_optional "typing.Optional"))
((typed_default_parameter
  name: (identifier) @ref.receiver.proof
  type: (type
    [(identifier) @ref.receiver.proof.type
     (generic_type (identifier) @ref.receiver.proof.type (type_parameter))
     (generic_type
       (identifier) @_optional
       (type_parameter . (type [(identifier) @ref.receiver.proof.type (string (string_content) @ref.receiver.proof.type)]) .))
     (subscript
       value: (attribute) @_typing_optional
       subscript: [(identifier) @ref.receiver.proof.type (string (string_content) @ref.receiver.proof.type)])
     (binary_operator left: (identifier) @ref.receiver.proof.type operator: "|" right: (none))
     (binary_operator left: (none) operator: "|" right: (identifier) @ref.receiver.proof.type)
     (string (string_content) @ref.receiver.proof.type)]))
  (#not-any-of? @ref.receiver.proof.type "Optional" "Union" "Annotated" "ClassVar" "Final")
  (#match? @ref.receiver.proof.type "^[A-Za-z_][A-Za-z0-9_]*$")
  (#eq? @_optional "Optional")
  (#eq? @_typing_optional "typing.Optional"))
((assignment
  left: (identifier) @ref.receiver.proof @ref.receiver.declared
  type: (type
    [(identifier) @ref.receiver.proof.type @ref.receiver.declared.type
     (generic_type (identifier) @ref.receiver.proof.type @ref.receiver.declared.type (type_parameter))
     (generic_type
       (identifier) @_optional
       (type_parameter . (type [(identifier) @ref.receiver.proof.type @ref.receiver.declared.type (string (string_content) @ref.receiver.proof.type @ref.receiver.declared.type)]) .))
     (subscript
       value: (attribute) @_typing_optional
       subscript: [(identifier) @ref.receiver.proof.type @ref.receiver.declared.type (string (string_content) @ref.receiver.proof.type @ref.receiver.declared.type)])
     (binary_operator left: (identifier) @ref.receiver.proof.type @ref.receiver.declared.type operator: "|" right: (none))
     (binary_operator left: (none) operator: "|" right: (identifier) @ref.receiver.proof.type @ref.receiver.declared.type)
     (string (string_content) @ref.receiver.proof.type @ref.receiver.declared.type)]))
  (#not-any-of? @ref.receiver.proof.type "Optional" "Union" "Annotated" "ClassVar" "Final")
  (#match? @ref.receiver.proof.type "^[A-Za-z_][A-Za-z0-9_]*$")
  (#eq? @_optional "Optional")
  (#eq? @_typing_optional "typing.Optional"))
((assignment
  left: (identifier) @ref.receiver.proof @ref.receiver.declared
  !type
  right: (call function: (identifier) @ref.receiver.proof.literal @ref.receiver.declared.type))
  (#match? @ref.receiver.proof.literal "^_*[A-Z]"))

((call
  function: (attribute
    object: (attribute
      object: (identifier) @_self
      attribute: (identifier) @ref.receiver.field)))
  (#eq? @_self "self"))

((assignment
  left: (attribute
    object: (identifier) @_self
    attribute: (identifier) @ref.receiver.declared)
  type: (type
    [(identifier) @ref.receiver.declared.type
     (generic_type (identifier) @ref.receiver.declared.type (type_parameter))
     (generic_type
       (identifier) @_optional
       (type_parameter . (type [(identifier) @ref.receiver.declared.type (string (string_content) @ref.receiver.declared.type)]) .))
     (subscript
       value: (attribute) @_typing_optional
       subscript: [(identifier) @ref.receiver.declared.type (string (string_content) @ref.receiver.declared.type)])
     (binary_operator left: (identifier) @ref.receiver.declared.type operator: "|" right: (none))
     (binary_operator left: (none) operator: "|" right: (identifier) @ref.receiver.declared.type)
     (string (string_content) @ref.receiver.declared.type)]))
  (#eq? @_self "self")
  (#not-any-of? @ref.receiver.declared.type "Optional" "Union" "Annotated" "ClassVar" "Final")
  (#match? @ref.receiver.declared.type "^[A-Za-z_][A-Za-z0-9_]*$")
  (#eq? @_optional "Optional")
  (#eq? @_typing_optional "typing.Optional"))
((assignment
  left: (attribute
    object: (identifier) @_self
    attribute: (identifier) @ref.receiver.declared)
  !type
  right: (call function: (identifier) @ref.receiver.declared.type))
  (#eq? @_self "self")
  (#match? @ref.receiver.declared.type "^_*[A-Z]"))
(class_definition
  body: (block
    (expression_statement
      (assignment
        left: [(identifier) @ref.receiver.declared @ref.receiver.declared.field
               (pattern_list (identifier) @ref.receiver.declared @ref.receiver.declared.field)
               (tuple_pattern (identifier) @ref.receiver.declared @ref.receiver.declared.field)]) @ref.receiver.declared.untyped)))
(class_definition
  body: (block
    [(function_definition
       name: (identifier) @ref.receiver.declared @ref.receiver.declared.field) @ref.receiver.declared.untyped
     (decorated_definition
       definition: (function_definition
         name: (identifier) @ref.receiver.declared @ref.receiver.declared.field)) @ref.receiver.declared.untyped
     (class_definition
       name: (identifier) @ref.receiver.declared @ref.receiver.declared.field) @ref.receiver.declared.untyped]))
((assignment
  left: [(attribute
           object: (identifier) @_self
           attribute: (identifier) @ref.receiver.declared @ref.receiver.declared.field)
         (pattern_list
           (attribute
             object: (identifier) @_self
             attribute: (identifier) @ref.receiver.declared @ref.receiver.declared.field))
         (tuple_pattern
           (attribute
             object: (identifier) @_self
             attribute: (identifier) @ref.receiver.declared @ref.receiver.declared.field))]) @ref.receiver.declared.untyped
  (#eq? @_self "self"))

(import_statement
  name: (dotted_name) @ref.import)

(import_statement
  name: (aliased_import
    name: (dotted_name) @ref.import
    alias: (identifier) @ref.import.alias))

(import_from_statement
  module_name: (_) @ref.import.from
  name: (dotted_name) @ref.import)

(import_from_statement
  module_name: (_) @ref.import.from
  name: (aliased_import
    name: (dotted_name) @ref.import
    alias: (identifier) @ref.import.alias))

(import_from_statement
  module_name: (_) @ref.import.from
  (wildcard_import) @ref.import @ref.import.asterisk)

;   @ref.access — an own-attribute access (`self.x`): a method reading an
;                 attribute of its own class (CR-005, FR-EX-08). The `#eq? self`
;                 predicate anchors the capture to the conventional receiver, so
;                 only own-member accesses are recorded. Resolution binds it to
;                 an `Accesses` edge only on an exactly-one Field candidate, else
;                 it stays unresolved (NFR-RA-05).
((attribute
  object: (identifier) @_recv
  attribute: (identifier) @ref.access)
  (#eq? @_recv "self"))

;   @ref.extends — each base the class header lists. The base is a name, a
;   dotted name, or the value of a subscript (a generic base).
(class_definition
  superclasses: (argument_list
    [(identifier) (attribute)] @ref.extends))
(class_definition
  superclasses: (argument_list
    (subscript value: [(identifier) (attribute)] @ref.extends)))

; ── Argument count (S-591, CR-190, FR-EX-32) ─────────────────────────────────
; The `@arity.*` vocabulary the extraction engine reads (`extract::arity`): every
; argument list, whose named children a call row counts, and the forms that
; make a count unknown. Captures record no row of their own, so every ledger
; target is unchanged.
; `*xs` and `**kw` spread; a lone generator (`f(x for x in xs)`) is one
; argument.
(argument_list) @arity.arguments
(argument_list (list_splat) @arity.spread)
(argument_list (dictionary_splat) @arity.spread)
(call
  arguments: (generator_expression) @arity.block)
; `cls.m(…)` may call a class method, whose `cls` is implicit, or an instance
; method, to which it passes the instance itself as the first argument: the
; count says nothing about `m`'s parameters (S-592).
(call
  function: (attribute object: (identifier) @_cls)
  arguments: (argument_list) @arity.unknown
  (#eq? @_cls "cls"))
