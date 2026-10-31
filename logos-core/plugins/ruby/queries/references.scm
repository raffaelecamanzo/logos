; Ruby reference-extraction query (S-059, capability = "references").
;
; The capture name carries the reference shape (see extract::collect_refs):
;   @ref.method — a method call (`obj.m()`, `Rails.application`, `helper(x)`);
;                 only the method name is knowable without type inference, so
;                 it binds by its receiver's SHAPE (the markers below; S-515,
;                 CR-169, FR-RS-12) — Ruby is dynamically typed, and a call the
;                 resolver cannot prove stays unresolved, never a guessed edge
;                 (NFR-RA-05). Ruby has no parentheses-free guarantee, so the
;                 fixtures call with `()` to surface a `call` node.
;   @ref.method.bare — a bare identifier in a value position (`to_app` in
;                 `to_app.call(env)`, a statement `unauthorized`): Ruby reads a
;                 name no local binds as a call with no receiver and no
;                 argument (S-568, FR-RS-35). Recorded only in a method, and
;                 only when no `@ref.access.shadow` of that method names it
;                 (`extract::bare_access`); then it is the receiver-less call
;                 `name()` is.
;   @ref.access.shadow — a name a method binds as a local: a parameter, a
;                 block parameter, an assignment's target, a `rescue => e`, a
;                 `for` variable, a pattern variable (the grammar's own
;                 `locals.scm` definitions). Records no row; read method-wide,
;                 so a local assigned after a same-named bare read hides it too
;                 — a lost call, never an invented one (NFR-RA-05).
;   @ref.access — an instance variable read or written (`@q`, `@q = …`,
;                 `@q ||= …`, `@a, @b = …`): a method's access to the state of
;                 its own class (S-569, FR-EX-27, FR-QM-11). `@x` is always the
;                 receiving object's, so no receiver anchors it; it names the
;                 Field `symbols.scm` declares (`@x`, S-564), and `plugin.toml`'s
;                 `member_access` is what lets Cohesion score the class from
;                 them. Resolution binds it to an `Accesses` edge only on an
;                 exactly-one Field candidate in the method's class, else it
;                 stays unresolved (NFR-RA-05). A module's methods bind
;                 nothing: their `@x` is whatever includes the module. A class
;                 body's, a `def self.x`'s and a `class << self` method's `@x`
;                 is the class object's own state, and binds to the same Field
;                 when the class declares that name once — a conflation of two
;                 objects' `@x` the graph cannot tell apart; a class body's is
;                 recorded from the class itself, which Cohesion ignores.
;   @ref.call   — a call on a constant (`User.find`, `A::B.find`), recorded
;                 with its constant (`@ref.call.scope`) as the Path
;                 `User::find` (S-565, FR-RS-33), bound among the constant's
;                 singleton methods and module functions.
;   @ref.extends — a class's superclass (`< ApplicationRecord`,
;                 `< ActiveRecord::Base`), read as a constant from the scope
;                 enclosing the class (S-568, FR-RS-35). Also the framework
;                 fingerprint the Rails detector reads (below).
;   @ref.implements — a module a class or module body mixes in: `include M`,
;                 and `prepend M` / `extend M`, marked by
;                 `@ref.implements.prepend` / `@ref.implements.extend` on the
;                 method name (S-568, FR-RS-35). Bound through the constant
;                 index to the one module the constant declares.
;   @ref.instantiates — the constant of `Foo.new` / `A::B.new` (S-565).
;   @ref.type_use — the scope of a compact `class A::B` / `module A::B`: the
;                 constant it names, and the scope the declaration's own
;                 constant is read under (S-565).
;   @ref.import — a `require`/`require_relative` path string, canonicalised as
;                 a path specifier (S-566): the Imports row binds the file it
;                 loads, and its ledger form feeds the framework candidacy gate.
;   @ref.import.relative — marks a `require_relative` string, relative to the
;                 requiring file's directory (S-566). Records no row of its own.
;
; Droppable on disk at `.logos/plugins/ruby/queries/references.scm`
; (FR-PL-04, FR-PL-05).
;
; Deliberately NOT captured in v1 (documented limitations, S-059): metaprogramming
; (`define_method`, `send`, `method_missing`) — genuinely dynamic, left honestly
; unresolved (NFR-RA-05). Constants are read as Ruby reads them, by their lexical
; nesting (S-565), which is the name Zeitwerk's autoloading loads them by.

; A receiver-less call is a call on Ruby's IMPLICIT receiver, `self`: inside a
; class, the descriptor's `implicit_receiver = "self"` records it as a `self`
; call, which binds among the class's own members only — never a same-named
; top-level method, or an outer class's. Anywhere else (file scope, a `module`,
; a block outside any class) it is the free call, bound by scope, it always was.
; The `!receiver` negation is load-bearing: it keeps a receiver call (`obj.m()`)
; from reading as implicit.
(call
  !receiver
  method: (identifier) @ref.method @ref.receiver.implicit)

; Receiver method calls keep the bare method name — except a call on a
; constant path (`Foo.bar`, `A::B.bar`, `::A.bar`), recorded with its constant
; below. One regular expression decides both patterns, so every receiver call
; is captured by exactly one of them; a value constant (`URI_PARSER.escape`) is
; a constant path too, and binds only if a class or module of that name exists.
(call
  receiver: (_) @_receiver
  method: (identifier) @ref.method
  (#not-match? @_receiver "^(::)?[A-Z][A-Za-z0-9_]*(::[A-Z][A-Za-z0-9_]*)*$"))

; A call on a constant (S-565, CR-179, FR-RS-33): `Foo.bar` records the Path
; `Foo::bar`, bound among the singleton methods and module functions of the
; constant the lexical nesting names; `Foo.new` records `Instantiates` → `Foo`.
(call
  receiver: (_) @ref.call.scope
  method: (identifier) @ref.call
  (#match? @ref.call.scope "^(::)?[A-Z][A-Za-z0-9_]*(::[A-Z][A-Za-z0-9_]*)*$")
  (#not-eq? @ref.call "new"))
(call
  receiver: (_) @ref.instantiates
  method: (identifier) @_new
  (#match? @ref.instantiates "^(::)?[A-Z][A-Za-z0-9_]*(::[A-Z][A-Za-z0-9_]*)*$")
  (#eq? @_new "new"))

; The scope of a compact `class A::B` (S-565): the declaration's constant is
; `A::B`, read under the constant enclosing it.
(class name: (scope_resolution scope: (_) @ref.type_use))
(module name: (scope_resolution scope: (_) @ref.type_use))

;   Receiver markers (S-515, CR-169, FR-EX-13) — MARKERS, recording nothing on
;   their own. Each names the receiver of a method call: its captured node
;   shares the call's parent with the `@ref.method` name, and the call records
;   that SHAPE, which the binder dispatches on (FR-RS-12).
;
;   @ref.receiver.implicit — the receiver-less call above (see there).
;   @ref.receiver.self     — `self.m()`: binds among the enclosing class's own
;                            members only.
;   @ref.receiver.anonymous — a `module` body. A module is a mixin: its
;                            methods' `self` is whatever includes it, so it is
;                            never the class around it (rack's
;                            `Request::Helpers`). Inside one, a bare call stays
;                            the free call, bound by scope, it always was, and
;                            `self.m()` binds only to an `m` the module itself
;                            declares (`def self.escape … self.unescape`);
;                            otherwise it is `other` (a `super` too).
;                            Also the bodies where a class's `self` is not one
;                            of its instances — `def self.x` and `class << self`
;                            in a class (the class object), a `Struct.new` /
;                            `Class.new` block (another class): there `self.m()`
;                            is `other`, since the graph cannot tell a singleton
;                            method from an instance one, and a bare call stays
;                            the free call it was.
;   @ref.receiver.other    — every other receiver (`obj.m()`, `User.find`,
;                            `super.foo` — `foo` is called on what `super`
;                            RETURNED): never binds. `self` outranks it.
;   @ref.receiver.super    — the keyword `super` (bare, `super(x)`, or the
;                            `super` of `super.foo`): a call of the BASE's method
;                            of the ENCLOSING method's name. That name is the
;                            method's own `name`, so the method name is the
;                            recorded call and the marker is the method's body —
;                            its sibling — whenever a `super` sits up to three
;                            levels into it (`super`, `x = super(1)`,
;                            `if c then super end`); a deeper one (inside a
;                            block) records nothing. Binds only along the
;                            ancestor chain past the caller's own class — its
;                            included modules, then its proven superclass
;                            (S-568) — and never reaches the caller itself.
;                            In a method of a `module` or a `class << self` body
;                            (`anonymous`, below) it records `other`: unbound too.
(call receiver: (self) @ref.receiver.self)
(module body: (body_statement) @ref.receiver.anonymous)
(class
  body: (body_statement
    (singleton_method body: (body_statement) @ref.receiver.anonymous)))
(singleton_class body: (body_statement) @ref.receiver.anonymous)
((call
  receiver: (constant) @_new_on
  method: (identifier) @_new
  block: [(do_block) (block)] @ref.receiver.anonymous)
  (#any-of? @_new_on "Struct" "Class")
  (#eq? @_new "new"))
(call receiver: (_) @ref.receiver.other)
(method
  name: (identifier) @ref.method
  body: (body_statement
    [(super) (_ (super)) (_ (_ (super)))]) @ref.receiver.super)
(singleton_method
  name: (identifier) @ref.method
  body: (body_statement
    [(super) (_ (super)) (_ (_ (super)))]) @ref.receiver.super)

; A class's superclass (S-568, FR-RS-35): `class A < B` records `Extends` → `B`,
; read as Ruby reads it — a constant, looked up from the scope enclosing `A`
; (`resolve::binder`'s constant rung). It is also a framework fingerprint
; (FR-FW-04 ledger candidacy), beside a call on a constant (`Rails.application…`,
; recorded `Rails::application` above): an external base (`< ActiveRecord::Base`)
; never binds (NFR-RA-05), and every ledger row's target — bound or not — is
; read by the Rails detector at a segment boundary.
(superclass
  [(constant) (scope_resolution)] @ref.extends)

; A module mixed into a class or module body (S-568, FR-RS-35): `include M`,
; `prepend M` and `extend M` record `Implements` → `M`, one row per constant
; argument. Only a statement of the body itself: an `include` inside a method,
; a block or a `class << self` body mixes into something else. A `prepend`'s
; module precedes the class in its ancestors, and an `extend`'s methods are
; the object's own singleton methods, outside them: each is marked on its row.
(class
  body: (body_statement
    (call
      !receiver
      method: (identifier) @_mixin
      arguments: (argument_list [(constant) (scope_resolution)] @ref.implements))
    (#eq? @_mixin "include")))
(module
  body: (body_statement
    (call
      !receiver
      method: (identifier) @_mixin
      arguments: (argument_list [(constant) (scope_resolution)] @ref.implements))
    (#eq? @_mixin "include")))
(class
  body: (body_statement
    (call
      !receiver
      method: (identifier) @ref.implements.prepend
      arguments: (argument_list [(constant) (scope_resolution)] @ref.implements))
    (#eq? @ref.implements.prepend "prepend")))
(module
  body: (body_statement
    (call
      !receiver
      method: (identifier) @ref.implements.prepend
      arguments: (argument_list [(constant) (scope_resolution)] @ref.implements))
    (#eq? @ref.implements.prepend "prepend")))
(class
  body: (body_statement
    (call
      !receiver
      method: (identifier) @ref.implements.extend
      arguments: (argument_list [(constant) (scope_resolution)] @ref.implements))
    (#eq? @ref.implements.extend "extend")))
(module
  body: (body_statement
    (call
      !receiver
      method: (identifier) @ref.implements.extend
      arguments: (argument_list [(constant) (scope_resolution)] @ref.implements))
    (#eq? @ref.implements.extend "extend")))

; A bare identifier in a value position (S-568, FR-RS-35): the grammar parses
; `to_app` as an `identifier` whether it names a local or calls a method, so
; every value position is captured and the extraction engine keeps the ones no
; local of the method binds (`@ref.access.shadow`, below). A name in a
; declaring position — a parameter, an assignment's target, a method's own
; name, a call's method — is in none of these.
(body_statement (identifier) @ref.method.bare)
(block_body (identifier) @ref.method.bare)
(then (identifier) @ref.method.bare)
(else (identifier) @ref.method.bare)
(ensure (identifier) @ref.method.bare)
(begin (identifier) @ref.method.bare)
(parenthesized_statements (identifier) @ref.method.bare)
(call receiver: (identifier) @ref.method.bare)
(argument_list (identifier) @ref.method.bare)
(assignment right: (identifier) @ref.method.bare)
(operator_assignment right: (identifier) @ref.method.bare)
(optional_parameter value: (identifier) @ref.method.bare)
(keyword_parameter value: (identifier) @ref.method.bare)
(binary left: (identifier) @ref.method.bare)
(binary right: (identifier) @ref.method.bare)
(unary operand: (identifier) @ref.method.bare)
(conditional condition: (identifier) @ref.method.bare)
(conditional consequence: (identifier) @ref.method.bare)
(conditional alternative: (identifier) @ref.method.bare)
(if condition: (identifier) @ref.method.bare)
(unless condition: (identifier) @ref.method.bare)
(while condition: (identifier) @ref.method.bare)
(until condition: (identifier) @ref.method.bare)
(if_modifier body: (identifier) @ref.method.bare)
(if_modifier condition: (identifier) @ref.method.bare)
(unless_modifier body: (identifier) @ref.method.bare)
(unless_modifier condition: (identifier) @ref.method.bare)
(while_modifier condition: (identifier) @ref.method.bare)
(until_modifier condition: (identifier) @ref.method.bare)
(element_reference object: (identifier) @ref.method.bare)
(pair value: (identifier) @ref.method.bare)
(interpolation (identifier) @ref.method.bare)
(array (identifier) @ref.method.bare)
(case value: (identifier) @ref.method.bare)

; An instance variable (S-569, FR-EX-27): every `@x` is an access of the class
; around its method — a read, and the target of an assignment alike, since a
; Field's writers and readers are the links Cohesion joins methods through. A
; `@@x` class variable is a different node (`class_variable`) and is not read.
(instance_variable) @ref.access

; The locals a method binds (S-568), after the grammar's own `locals.scm`: each
; hides a same-named bare identifier anywhere in the method.
(method_parameters (identifier) @ref.access.shadow)
(lambda_parameters (identifier) @ref.access.shadow)
(block_parameters (identifier) @ref.access.shadow)
(block_parameter (identifier) @ref.access.shadow)
(splat_parameter (identifier) @ref.access.shadow)
(hash_splat_parameter (identifier) @ref.access.shadow)
(destructured_parameter (identifier) @ref.access.shadow)
(optional_parameter name: (identifier) @ref.access.shadow)
(keyword_parameter name: (identifier) @ref.access.shadow)
(assignment left: (identifier) @ref.access.shadow)
(operator_assignment left: (identifier) @ref.access.shadow)
(left_assignment_list (identifier) @ref.access.shadow)
(rest_assignment (identifier) @ref.access.shadow)
(destructured_left_assignment (identifier) @ref.access.shadow)
(exception_variable (identifier) @ref.access.shadow)
(for pattern: (identifier) @ref.access.shadow)
(as_pattern name: (identifier) @ref.access.shadow)
(in_clause pattern: (identifier) @ref.access.shadow)
(array_pattern (identifier) @ref.access.shadow)
(find_pattern (identifier) @ref.access.shadow)
(keyword_pattern value: (identifier) @ref.access.shadow)
; `in {name:}` binds `name` from its key alone; `h => x` and `h in x` bind `x`.
(keyword_pattern !value key: (hash_key_symbol) @ref.access.shadow)
(match_pattern pattern: (identifier) @ref.access.shadow)
(test_pattern pattern: (identifier) @ref.access.shadow)
; A block's implicit parameters — `it` (Ruby 3.4) and `_1` … `_9` — are locals of
; the block, written nowhere as a parameter: any of them in a method hides it.
((identifier) @ref.access.shadow
  (#match? @ref.access.shadow "^(it|_[1-9])$"))

; `require "active_support"` / `require_relative "../user"` import paths
; (S-566, FR-RS-34), recorded as path specifiers (`plugin.toml`). A
; `require_relative` string is relative to the requiring file's directory
; whatever it spells — `'abstract/handler'` is `./abstract/handler` — so it is
; marked `@ref.import.relative`; a bare `require` (and every other
; `require*` call, as before) binds against the load roots. The `#not-eq?`
; keeps the two patterns disjoint, so each require is captured once.
(call
  !receiver
  method: (identifier) @_req
  arguments: (argument_list
    .
    (string) @ref.import)
  (#match? @_req "^require")
  (#not-eq? @_req "require_relative"))
(call
  !receiver
  method: (identifier) @_req
  arguments: (argument_list
    .
    (string) @ref.import @ref.import.relative)
  (#eq? @_req "require_relative"))

; ── Argument count (S-591, CR-190, FR-EX-32) ─────────────────────────────────
; The `@arity.*` vocabulary the extraction engine reads (`extract::arity`): every
; argument list, whose named children a call row counts, and the forms that
; make a count unknown. Captures record no row of their own, so every ledger
; target is unchanged.
; `*xs`, `**h` and `...` spread, and so does a bare `key: value` run; a
; `&block` is no argument; a call written
; without an argument list (`x.m`) passes none; a call's `receiver` is not its
; callee.
(argument_list) @arity.arguments
(argument_list (splat_argument) @arity.spread)
(argument_list (hash_splat_argument) @arity.spread)
(argument_list (forward_argument) @arity.spread)
(argument_list (block_argument) @arity.skip)
; A bare `key: value` run is one positional hash when the callee declares no
; keyword parameter, and one argument per key when it does — the count depends
; on the callee, so it is unknown.
(argument_list (pair) @arity.spread)
(call
  method: (_)
  !arguments) @arity.none
(call receiver: (_) @arity.receiver)
