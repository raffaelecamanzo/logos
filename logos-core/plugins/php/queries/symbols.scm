; PHP symbol-extraction query (S-060, capability = "symbols").
;
; Capture names map to NodeKind via NodeKind::as_str (extract engine).
; Compiled against the built PHP Language on its first use, or at
; load as an on-disk override; drift fails naming this file (FR-PL-02,
; CR-197). Droppable on disk at
; `.logos/plugins/php/queries/symbols.scm` (FR-PL-04, UAT-PL-03).
;
; Classes map to NodeKind::Class, which is what makes Cohesion (LCOM4)
; applicable for PHP (ADR-21 declarative applicability: the kind a construct
; extracts to *is* its declared applicability). Interfaces/traits/enums map to
; their own kinds and are correctly excluded from cohesion scope.
;
; v1 policy: `__construct` is captured like any other method — its name never
; collides with the class name (unlike a Java constructor), so a `new Class()`
; is an `Instantiates` row bound to the class itself, never to its constructor
; (references.scm, S-547).

(class_declaration
  name: (name) @symbol.class)

(interface_declaration
  name: (name) @symbol.interface)

(trait_declaration
  name: (name) @symbol.trait)

(enum_declaration
  name: (name) @symbol.enum)

(method_declaration
  name: (name) @symbol.method)

(function_definition
  name: (name) @symbol.function)

; A typed-or-untyped class property: `public int $balance = 0;`. The captured
; name is the inner `name` of the `$variable` (`balance`), so a `$this->balance`
; own-field access (references.scm) binds to it for LCOM4 (FR-EX-08).
(property_declaration
  (property_element
    name: (variable_name (name) @symbol.field)))

; A constructor-promoted parameter is a property of its class
; (`public function __construct(private Logger $log)`, S-548, FR-EX-21): a Field
; named `log`, so `$this->log` binds to it like a declared property. Its
; declaration is the `$log` variable, which `assign_parents` lifts out of the
; constructor to the class (the parameter list is the method's `parameters`
; field). A by-reference promotion (`private array &$rows`) wraps the variable in
; `by_ref`; the name is the same. The match is anchored to a method of a named
; class or trait, the only types that have a constructor to promote in: an
; anonymous class is not a declaration, so its constructor's Field would be
; owned by the enclosing function, and a promotion in a plain function or
; closure is not valid PHP.
(class_declaration
  body: (declaration_list
    (method_declaration
      parameters: (formal_parameters
        (property_promotion_parameter
          name: [(variable_name (name) @symbol.field)
                 (by_ref (variable_name (name) @symbol.field))])))))
(trait_declaration
  body: (declaration_list
    (method_declaration
      parameters: (formal_parameters
        (property_promotion_parameter
          name: [(variable_name (name) @symbol.field)
                 (by_ref (variable_name (name) @symbol.field))])))))

; A class, interface, trait or enum constant (S-548, FR-EX-21): a Constant under
; its type, one per element of `const A = 1, B = 2;`. The name is the element's
; FIRST child (`const A = B;` has a second `name`, its value, which is no
; declaration), and the match is anchored to a named type's body, so a global
; `const X = 1;` and an anonymous class's constant are not captured.
(class_declaration
  body: (declaration_list
    (const_declaration (const_element . (name) @symbol.constant))))
(interface_declaration
  body: (declaration_list
    (const_declaration (const_element . (name) @symbol.constant))))
(trait_declaration
  body: (declaration_list
    (const_declaration (const_element . (name) @symbol.constant))))
(enum_declaration
  body: (enum_declaration_list
    (const_declaration (const_element . (name) @symbol.constant))))

; An enum case is a Constant under its enum (S-548, FR-EX-21).
(enum_declaration
  body: (enum_declaration_list
    (enum_case name: (name) @symbol.constant)))

; The file's namespace (S-518, CR-170, FR-RS-13). Not a declaration — its
; capture group is `module`, not `symbol`, so the declaration walk skips it. The
; statement form (`namespace Monolog\Handler;`) scopes the rest of the file, the
; braced form (`namespace App { … }`) its body; each top-level type is named
; that namespace plus its own name, whatever directory the file sits in.
(namespace_definition
  name: (namespace_name) @module.namespace)

; ── Parameter range (S-591, CR-190, FR-EX-32) ────────────────────────────────
; The `@arity.*` vocabulary the extraction engine reads (`extract::arity`): a
; callable's parameter list, and each parameter as required, optional (a
; default value raises only the maximum), variadic (an unbounded maximum), a
; receiver (not counted) or no parameter at all (skip). A list child no capture
; covers, or one captured `@arity.unknown`, records the range unknown rather
; than miscounting it. Captures never name a declaration, so every symbol and
; node is unchanged.
; A parameter with a default is optional; `...$xs` is variadic. A user-defined
; function accepts more arguments than it declares (`func_get_args()` reads
; them), so every list is also captured variadic: the minimum is the required
; parameters, the maximum unbounded.
(function_definition
  parameters: (formal_parameters) @arity.parameters @arity.variadic)
(method_declaration
  parameters: (formal_parameters) @arity.parameters @arity.variadic)
(formal_parameters (simple_parameter) @arity.required)
(formal_parameters (simple_parameter default_value: (_)) @arity.optional)
(formal_parameters (variadic_parameter) @arity.variadic)
(formal_parameters (property_promotion_parameter) @arity.required)
(formal_parameters (property_promotion_parameter default_value: (_)) @arity.optional)
