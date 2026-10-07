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
; reference stays an honest unresolved class reference, not an ambiguous one.

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
