; Ruby reference-extraction query (S-059, capability = "references").
;
; The capture name carries the reference shape (see extract::collect_refs):
;   @ref.call   — a receiver-less call (`callee()`, `helper(x)`); the text is
;                 the path. The `!receiver` negation is load-bearing: it keeps a
;                 receiver method call (`obj.m()`) out of the Path arm, so a bare
;                 name binds as a same-scope call but a member call never
;                 fabricates an edge to an unrelated same-named top-level method
;                 (NFR-RA-05). Ruby has no parentheses-free guarantee, so the
;                 fixtures call with `()` to surface a `call` node.
;   @ref.method — a receiver method call (`obj.m()`, `Rails.application`); only
;                 the method name is knowable without type inference, so binding
;                 is policy-gated (Ruby is dynamically typed — dynamic dispatch
;                 the resolver cannot prove stays unresolved, never a guessed
;                 edge).
;   @ref.import — a `require`/`require_relative` path string, canonicalised into
;                 the ledger form that feeds the framework candidacy gate.
;
; Droppable on disk at `.logos/plugins/ruby/queries/references.scm`
; (FR-PL-04, FR-PL-05).
;
; Deliberately NOT captured in v1 (documented limitations, S-059): metaprogramming
; (`define_method`, `send`, `method_missing`) and constant autoloading — all
; genuinely dynamic, left honestly unresolved (NFR-RA-05).

; Receiver-less calls bind as Calls paths.
(call
  !receiver
  method: (identifier) @ref.call)

; Receiver method calls keep the bare method name (policy-gated member form).
(call
  receiver: (_)
  method: (identifier) @ref.method)

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
