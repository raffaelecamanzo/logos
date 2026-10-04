; Go reference-extraction query (S-015, capability = "references").
;
;   @ref.call   — a plain-identifier call (`f()`).
;   @ref.method — a selector call (`pkg.F()`, `recv.M()`). A call on a value
;                 (`recv.M()`) stays name-only and carries its receiver's shape
;                 (below, S-517); a call whose operand is an imported package
;                 (`pkg.F()`) is recorded qualified by its import path by the
;                 extraction engine itself (S-440).
;   @ref.receiver.other / @ref.receiver.self_name — the receiver shape of a
;                 selector call (S-517, CR-169, FR-EX-13; the vocabulary is
;                 S-514's). Go has no `this`: a method names its own instance by
;                 the receiver parameter, so `self_name` captures that identifier
;                 and `other` captures every selector operand. The extraction
;                 engine reads an `other` operand whose text is the enclosing
;                 method's `self_name` as `self` — `s.F()` inside
;                 `func (s *Svc) …` is a call on the method's own receiver and
;                 binds among `Svc`'s methods (through the self type
;                 `symbols.scm` records, S-509); `x.F()`, `s.next.F()` and a
;                 parameter that happens to be called `s` in a free function are
;                 `other` and bind nowhere, never to a free `func F`. A bare
;                 `F()` is `@ref.call` and still binds through the scope walk.
;                 Known gap: `self` is the operand's *text* equalling the
;                 receiver's name, so a name the method rebinds (`for _, s :=
;                 range …`, `s := …`, a closure parameter) still reads as the
;                 receiver (pinned in `tests/receiver_shape_go_rust.rs`).
;   @ref.import — an import path string (`import "net/http"`); unquoted and
;                 canonicalised by the PATH grammar the descriptor declares
;                 (only slashes → `::`; a host's dots are kept, S-439) into
;                 the ledger form feeding the binder and the framework
;                 candidacy gate (FR-FW-04).
;
; Droppable on disk at `.logos/plugins/go/queries/references.scm`.

(call_expression
  function: (identifier) @ref.call)

(call_expression
  function: (selector_expression
    field: (field_identifier) @ref.method))

; The marker and the call's `@ref.method` node share one parent — the
; `selector_expression` — which is how the engine pairs them. An unnamed
; receiver (`func (*Svc) M()`, `func (_ Svc) M()`) has no usable name to capture,
; so nothing inside such a method is `self`.
(selector_expression
  operand: (_) @ref.receiver.other)

(method_declaration
  receiver: (parameter_list
    (parameter_declaration
      name: (identifier) @ref.receiver.self_name)))

(import_spec
  path: (interpreted_string_literal) @ref.import)
