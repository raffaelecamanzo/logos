; Go symbol-extraction query (S-015, capability = "symbols").
;
; Capture names map to NodeKind via NodeKind::as_str (extract engine).
; Compiled against the built Go Language at load; fails fast naming this file
; on drift (FR-PL-02). Droppable on disk at
; `.logos/plugins/go/queries/symbols.scm` (FR-PL-04, UAT-PL-03).
;
; v1 policy: struct and interface type specs are captured with their concrete
; kinds; other type declarations (aliases, defined non-struct types) are a
; later increment.

(function_declaration
  name: (identifier) @symbol.function)

; The plain pattern keeps the method's kind and symbol; the four after it name the
; same `method_declaration` once more with `@symbol.self_type` (S-509, FR-EX-12).
(method_declaration
  name: (field_identifier) @symbol.method)

; A method's receiver base type (S-509, S-493's capture): the receiver's
; `type_identifier`, with the pointer star and the type-argument list OUTSIDE the
; capture — `func (s *Svc[T]) Work()` and `func (s Svc) Rest()` both record `Svc`.
; The receiver name is optional and never captured, so `func (*Svc) M()`,
; `func (Svc) M()` and `func (_ Svc) M()` record it too. The extraction engine
; gives the self type to the declaration the same match captures (and takes the
; declaration once, whichever pattern names it first), so symbols and kinds are
; unchanged; it is persisted beside the node (`nodes.self_type`) in the form the
; Rust plugin's impl self type uses. A free `func` has no receiver and records
; none. Go spells no qualified receiver (`pkg.T` is not a receiver type), so
; there is nothing to refuse here; the one form left out is the parenthesised
; `func (s (*Svc)) M()`, which no code writes.
(method_declaration
  receiver: (parameter_list
    (parameter_declaration
      type: (type_identifier) @symbol.self_type))
  name: (field_identifier) @symbol.method)

(method_declaration
  receiver: (parameter_list
    (parameter_declaration
      type: (generic_type
        type: (type_identifier) @symbol.self_type)))
  name: (field_identifier) @symbol.method)

(method_declaration
  receiver: (parameter_list
    (parameter_declaration
      type: (pointer_type
        (type_identifier) @symbol.self_type)))
  name: (field_identifier) @symbol.method)

(method_declaration
  receiver: (parameter_list
    (parameter_declaration
      type: (pointer_type
        (generic_type
          type: (type_identifier) @symbol.self_type))))
  name: (field_identifier) @symbol.method)

(type_declaration
  (type_spec
    name: (type_identifier) @symbol.struct
    type: (struct_type)))

(type_declaration
  (type_spec
    name: (type_identifier) @symbol.interface
    type: (interface_type)))

(const_declaration
  (const_spec
    name: (identifier) @symbol.constant))

(var_declaration
  (var_spec
    name: (identifier) @symbol.variable))
