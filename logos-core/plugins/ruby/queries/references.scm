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
;   @ref.call   — a constant (`< ApplicationController`, `User.find`): a free
;                 framework fingerprint (below).
;   @ref.import — a `require`/`require_relative` path string, canonicalised into
;                 the ledger form that feeds the framework candidacy gate.
;
; Droppable on disk at `.logos/plugins/ruby/queries/references.scm`
; (FR-PL-04, FR-PL-05).
;
; Deliberately NOT captured in v1 (documented limitations, S-059): metaprogramming
; (`define_method`, `send`, `method_missing`) and constant autoloading — all
; genuinely dynamic, left honestly unresolved (NFR-RA-05).

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

; Receiver method calls keep the bare method name.
(call
  receiver: (_)
  method: (identifier) @ref.method)

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
;                            block) records nothing. Binds only through a proven
;                            `Extends`, which this plugin does not record — so it
;                            stays unbound, and never reaches the caller itself.
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

; Framework fingerprints (FR-FW-04 ledger candidacy): a class/module superclass
; (`< ApplicationController`, `< ActiveRecord::Base`) and a constant call
; receiver (`Rails.application…`, `User.find`). Captured as Calls paths; an
; external constant never binds (NFR-RA-05), so the surviving ledger entry is a
; free framework fingerprint the Rails detector reads.
(superclass
  [(constant) (scope_resolution)] @ref.call)

(call
  receiver: (constant) @ref.call)

; `require "active_support"` / `require_relative "../user"` import paths.
(call
  !receiver
  method: (identifier) @_req
  arguments: (argument_list
    .
    (string) @ref.import)
  (#match? @_req "^require"))
