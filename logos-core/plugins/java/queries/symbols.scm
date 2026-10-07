; Java symbol-extraction query (S-015, capability = "symbols").
;
; Capture names map to NodeKind via NodeKind::as_str (extract engine).
; Compiled against the built Java Language on its first use, or at
; load as an on-disk override; drift fails naming this file (FR-PL-02,
; CR-197). Droppable on disk at
; `.logos/plugins/java/queries/symbols.scm` (FR-PL-04, UAT-PL-03).
;
; v1 policy: constructors are deliberately NOT captured — a constructor
; shares its class's name, and a second same-named node would make every
; `ClassName` reference ambiguous under the binder's exactly-one-or-nothing
; rule (NFR-RA-05). `new ClassName()` is recorded as an `Instantiates` of the
; class itself (references.scm, S-466).

(class_declaration
  name: (identifier) @symbol.class)

(record_declaration
  name: (identifier) @symbol.class)

(interface_declaration
  name: (identifier) @symbol.interface)

(enum_declaration
  name: (identifier) @symbol.enum)

(method_declaration
  name: (identifier) @symbol.method)

(field_declaration
  declarator: (variable_declarator
    name: (identifier) @symbol.field))

; The file's `package` statement (S-472). Not a declaration — its capture group
; is `package`, not `symbol`, so the declaration walk skips it; the declared-type
; reader compares it with the package the file's directory keys it by, and a
; disagreement is recorded refused rather than resolved to the path.
(package_declaration
  [(identifier) (scoped_identifier)] @package.name)

; ── Parameter range (S-591, CR-190, FR-EX-32) ────────────────────────────────
; The `@arity.*` vocabulary the extraction engine reads (`extract::arity`): a
; callable's parameter list, and each parameter as required, optional (a
; default value raises only the maximum), variadic (an unbounded maximum), a
; receiver (not counted) or no parameter at all (skip). A list child no capture
; covers, or one captured `@arity.unknown`, records the range unknown rather
; than miscounting it. Captures never name a declaration, so every symbol and
; node is unchanged.
; A receiver parameter (`C this`) is not counted; `T... xs` is variadic.
(method_declaration
  parameters: (formal_parameters) @arity.parameters)
(formal_parameters (formal_parameter) @arity.required)
(formal_parameters (spread_parameter) @arity.variadic)
(formal_parameters (receiver_parameter) @arity.receiver)
