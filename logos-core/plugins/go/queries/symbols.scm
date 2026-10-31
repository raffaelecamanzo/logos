; Go symbol-extraction query (S-015, capability = "symbols").
;
; Capture names map to NodeKind via NodeKind::as_str (extract engine).
; Compiled against the built Go Language on its first use, or at
; load as an on-disk override; drift fails naming this file (FR-PL-02,
; CR-197). Droppable on disk at
; `.logos/plugins/go/queries/symbols.scm` (FR-PL-04, UAT-PL-03).
;
; Declarations (S-525, CR-172, FR-EX-15): every top-level type, top-level
; `var`/`const`, interface method spec and struct field is a node.
;   - a struct is a Struct and an interface an Interface wherever it is declared
;     (those patterns are unanchored, so every symbol they named is unchanged);
;   - any other TOP-LEVEL type — a defined type (`type Level int8`) or an alias
;     (`type A = B`) — is a TypeAlias, the alias marked `@item.alias`;
;   - a top-level `var`/`const`, single or grouped, is a Variable/Constant for
;     EVERY name it declares (`var a, b = 1, 2`); one declared in a function body
;     is none — the one removal of pre-existing symbols, as FR-EX-15 states;
;   - an interface's method specs are bodyless Methods (`@item.signature`);
;   - a struct's fields are Fields, one per name (`x, y int`), an embedded field
;     one named for the embedded type. Only the struct a `type` declares has
;     them: an anonymous `struct{…}` is no declaration, so its fields are none.

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

; A defined type that is neither struct nor interface. The type is listed by
; kind because a query cannot say "any type but these"; a form the grammar adds
; later is missed until it is listed here, never mis-kinded.
(source_file
  (type_declaration
    (type_spec
      name: (type_identifier) @symbol.type_alias
      type: [
        (type_identifier) (qualified_type) (generic_type) (pointer_type)
        (array_type) (implicit_length_array_type) (slice_type) (map_type)
        (channel_type) (function_type) (parenthesized_type) (negated_type)
      ])))

; An alias: tree-sitter-go gives `type A = B` its own `type_alias` node.
(source_file
  (type_declaration
    (type_alias
      name: (type_identifier) @symbol.type_alias) @item.alias))

; Interface method specs. A spec's parameter range is captured below with the
; callables' (`@arity.parameters`): the method-set pass matches a type's methods
; against an interface's by name and parameter count (S-529, FR-RS-18).
(type_spec
  type: (interface_type
    (method_elem
      name: (field_identifier) @symbol.method) @item.signature))

; Struct fields. The child form `(field_identifier)` matches every name of
; `x, y int`. A match that declares several names on one node carries
; `@symbol.names` (here and on the `var`/`const` patterns below): the extraction
; engine then records each name as a declaration of its own, where it records
; only the first name of any other multi-name declaration.
(type_spec
  type: (struct_type
    (field_declaration_list
      (field_declaration
        (field_identifier) @symbol.field @symbol.names))))

; An embedded field has no name node: the embedded type names it. `*T` is the
; star token and the type; a package-qualified or generic type names it by its
; own name.
(type_spec
  type: (struct_type
    (field_declaration_list
      (field_declaration
        !name
        type: (type_identifier) @symbol.field))))

(type_spec
  type: (struct_type
    (field_declaration_list
      (field_declaration
        !name
        type: (qualified_type
          name: (type_identifier) @symbol.field)))))

(type_spec
  type: (struct_type
    (field_declaration_list
      (field_declaration
        !name
        type: (generic_type
          type: (type_identifier) @symbol.field)))))

(type_spec
  type: (struct_type
    (field_declaration_list
      (field_declaration
        !name
        type: (generic_type
          type: (qualified_type
            name: (type_identifier) @symbol.field))))))

; Top-level `var` / `const`. The child form `(identifier)` matches every name of
; a multi-name spec, and only names: the type is a `type_identifier` and the
; value an `expression_list`. The first name owns the initialiser's calls and
; the types nested in it. tree-sitter-go 0.25 wraps a grouped `var (…)` in a
; `var_spec_list`; a grouped `const (…)` holds its specs directly.
(source_file
  (const_declaration
    (const_spec
      (identifier) @symbol.constant @symbol.names)))

(source_file
  (var_declaration
    (var_spec
      (identifier) @symbol.variable @symbol.names)))

(source_file
  (var_declaration
    (var_spec_list
      (var_spec
        (identifier) @symbol.variable @symbol.names))))

; The file's `package` clause (S-528, CR-172, FR-RS-17). Not a declaration — its
; capture group is `package`, not `symbol`, so the declaration walk skips it. The
; descriptor declares `directory_packages`, so extraction records the clause
; beside the file, and the binder's same-package rung reads it: the files of one
; directory declaring one clause share one package scope, and a `package foo_test`
; file is a package of its own.
(source_file
  (package_clause
    (package_identifier) @package.name))

; ── Parameter range (S-591, CR-190, FR-EX-32) ────────────────────────────────
; The `@arity.*` vocabulary the extraction engine reads (`extract::arity`): a
; callable's parameter list, and each parameter as required, optional (a
; default value raises only the maximum), variadic (an unbounded maximum), a
; receiver (not counted) or no parameter at all (skip). A list child no capture
; covers, or one captured `@arity.unknown`, records the range unknown rather
; than miscounting it. Captures never name a declaration, so every symbol and
; node is unchanged.
; A method's receiver sits outside its `parameters` list and is not counted.
; `a, b int` is one declaration naming two parameters: each name is captured.
(function_declaration
  parameters: (parameter_list) @arity.parameters)
(method_declaration
  parameters: (parameter_list) @arity.parameters)
; An interface's method spec (S-529): its list holds no receiver either. A spec of
; a generic interface (`type G[T any] interface { Get(T) }`) records its range
; unknown: its parameter types wait on the interface's type arguments, so a count
; matched against a concrete method proves nothing, and the method-set pass
; reports such an interface not computed (FR-RS-18).
(method_elem
  parameters: (parameter_list) @arity.parameters)
(type_spec
  type_parameters: (type_parameter_list)
  type: (interface_type
    (method_elem
      parameters: (parameter_list) @arity.unknown)))
(parameter_list (parameter_declaration name: (_) @arity.required))
(parameter_list (parameter_declaration !name) @arity.required)
(parameter_list (variadic_parameter_declaration) @arity.variadic)
