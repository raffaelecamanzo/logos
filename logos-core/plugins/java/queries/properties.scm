; Java configuration-binding capture (S-381, CR-121, capability = "properties",
; FR-WS-19, FR-PL-02).
;
; Captures the *anchors* of a `@ConfigurationProperties` class — its annotation
; name, its key prefix, its simple name and the properties it declares — and
; leaves every judgment to the generic interpreter
; (`extract::config::binding::PropertiesIndex`), exactly as `invocations.scm`
; leaves the HTTP-verb and static-literal decisions to its dispatch (NFR-MA-01).
;
;   @props.class      — the declaration node. The GROUPING KEY, never read for
;                       text: no grammar can bind a class and all of its fields
;                       in one match, so the patterns below bind it repeatedly
;                       and the interpreter merges them.
;   @props.class.name — the node whose text is the class's simple name.
;   @props.annotation — the node whose text is an annotation's name. Kept only
;                       when `[properties] annotations` lists it.
;   @props.prefix     — the node holding the key prefix. Kept only when it is a
;                       static string literal; a constant reference or a
;                       concatenation leaves the class prefix-LESS and counted,
;                       never keyed on a guess (NFR-RA-05).
;   @props.field      — the node whose text is one declared property's name.
;
; ── The vocabulary is NOT in this file, on purpose ───────────────────────────
;
; There is no `(#eq? @props.annotation "ConfigurationProperties")` below, and its
; absence is the design rather than an omission. The annotation vocabulary lives
; in `plugin.toml`'s `[properties] annotations`, which the interpreter matches
; EXACTLY against this capture's text. A predicate here would be a second copy of
; that list, free to drift from it — and the drifted copy would be the silent
; one, because a query that matches nothing and a vocabulary that admits nothing
; are indistinguishable at the product layer. So this file answers "what shape is
; a bound class in Java", and the descriptor answers "which annotation marks
; one".
;
; ── Patterns ─────────────────────────────────────────────────────────────────

; 1. The annotated declaration itself, in every annotation form — with
;    arguments, or a bare marker. This is the pattern that makes a class VISIBLE;
;    patterns 2-3 only add its prefix, and 4-6 only add its properties.
;      @ConfigurationProperties(prefix = "mailserver.api")
;      public class MailServerConfigurationApi { … }
(class_declaration
  (modifiers [
    (annotation name: (identifier) @props.annotation)
    (marker_annotation name: (identifier) @props.annotation)
  ])
  name: (identifier) @props.class.name) @props.class

(record_declaration
  (modifiers [
    (annotation name: (identifier) @props.annotation)
    (marker_annotation name: (identifier) @props.annotation)
  ])
  name: (identifier) @props.class.name) @props.class

; 2. The NAMED prefix argument — `@ConfigurationProperties(prefix = "x.y")`, the
;    form the reference estate overwhelmingly uses. `value` is Spring's alias for
;    `prefix`, so both argument names are read; any other named argument
;    (`ignoreUnknownFields = …`) binds nothing.
;
;    WHICH ARGUMENT carries the prefix is Java syntax, so it IS decided here —
;    unlike WHICH ANNOTATION binds, which is descriptor data. The two are
;    different questions and they live in different files.
;
;    The annotation's NAME is bound in the same match, and that is not
;    decoration: a bound class routinely also carries `@Validated`,
;    `@Component("bean")` or `@RequestMapping(value = "/x")`, and a prefix read
;    per CLASS rather than per ANNOTATION takes one of those as the
;    configuration prefix. The interpreter keeps a prefix only when its own match
;    binds an in-vocabulary annotation, so this capture is what makes the pattern
;    say which annotation it is reading.
((class_declaration
  (modifiers
    (annotation
      name: (identifier) @props.annotation
      arguments: (annotation_argument_list
        (element_value_pair
          key: (identifier) @_key
          value: (_) @props.prefix))))) @props.class
 (#any-of? @_key "prefix" "value"))

((record_declaration
  (modifiers
    (annotation
      name: (identifier) @props.annotation
      arguments: (annotation_argument_list
        (element_value_pair
          key: (identifier) @_key
          value: (_) @props.prefix))))) @props.class
 (#any-of? @_key "prefix" "value"))

; 3. The single-value form — `@ConfigurationProperties("x.y")`. Anchored to the
;    argument list's FIRST named child so it cannot also match a named
;    argument's value, which sits at the same depth; and binding the annotation
;    name for the same reason pattern 2 does.
(class_declaration
  (modifiers
    (annotation
      name: (identifier) @props.annotation
      arguments: (annotation_argument_list
        .
        (string_literal) @props.prefix)))) @props.class

(record_declaration
  (modifiers
    (annotation
      name: (identifier) @props.annotation
      arguments: (annotation_argument_list
        .
        (string_literal) @props.prefix)))) @props.class

; 4. Declared fields — the properties of a setter-bound class. A DIRECT child of
;    the class body, which is what keeps a nested type's fields out: they bind to
;    the nested `class_declaration`, which carries no binding annotation of its
;    own and is therefore dropped. A nested accessor (`a.getB().getC()`) is a
;    stated ceiling of the resolution half, not of this capture.
;
;    One match per declarator, so `private int timeout, retries;` yields both.
(class_declaration
  body: (class_body
    (field_declaration
      declarator: (variable_declarator
        name: (identifier) @props.field)))) @props.class

; 5. A record's own components — Spring 3 constructor binding.
;      @ConfigurationProperties("other.api")
;      record OtherApi(String baseUrl, int port) { }
;    Restricted to the record's `parameters:` list rather than to every
;    `formal_parameter` in the declaration: unrestricted, a setter's or helper's
;    parameter registered as a declared property, which loosens the
;    `PropertyNotDeclared` refusal and — wherever a source happens to define
;    `<prefix>.<paramName>` — admits a site outright.
(record_declaration
  parameters: (formal_parameters
    (formal_parameter
      name: (identifier) @props.field))) @props.class

; 6. A record's body fields (statics, and the compact-constructor form's
;    assignments' targets). Carried for parity with pattern 4 so the two
;    declaration forms declare properties by the same rule.
(record_declaration
  body: (class_body
    (field_declaration
      declarator: (variable_declarator
        name: (identifier) @props.field)))) @props.class

; ── Stated coverage ceilings (ADR-54: recorded, never worked around) ─────────
;
; NOT captured:
;
;   * A fully-qualified annotation
;     (`@org.springframework.boot.context.properties.ConfigurationProperties`).
;     Java spells it `(scoped_identifier)` where pattern 1 requires an
;     `(identifier)`, which is the same spelling the S-365 harness required, so
;     this ceiling is inherited rather than introduced. Closing it is one
;     alternation here plus one descriptor row — the vocabulary compares the
;     captured text, so the qualified name is just another row.
;   * `@ConfigurationProperties` on a `@Bean` FACTORY METHOD. The prefix is real
;     but the bound type is the method's return type, which needs type
;     resolution this capture does not do. Such a class reaches the interpreter
;     through no pattern at all and is invisible rather than mis-keyed.
;   * A prefix that is not a written literal — a constant reference or a
;     concatenation. Pattern 2 captures the argument node whatever its shape, and
;     the interpreter's shared literal reader declines it, so the class is
;     counted under `PropertiesIndex::prefixless` rather than dropped silently.
;   * A property reached through a NESTED properties type
;     (`config.getMail().getHost()`). Pattern 4's direct-child rule attributes
;     the nested type's fields to the nested type, and the resolution half
;     refuses the chained accessor.
;
; Like every capability query this file is droppable-on-disk: a copy at
; `.logos/plugins/java/queries/properties.scm` shadows it without a rebuild
; (FR-PL-04, FR-PL-05).
