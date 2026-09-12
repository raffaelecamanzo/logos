; Kotlin configuration-binding capture (S-381, CR-121, capability =
; "properties", FR-WS-19, FR-PL-02).
;
; The SECOND language on the binding substrate, and the reason it exists: adding
; it cost this file, a `[properties]` table and two `grammars.rs` rows, and NOT
; one line of `logos-core` (S-381 AC2). The capture contract is stated once, in
; `plugins/java/queries/properties.scm` and in
; `extract::config::binding`'s module docs — read either for what the five
; capture names mean. This header states only what is Kotlin about Kotlin.
;
; ── Why a Java-shaped indexer could not have been reused ─────────────────────
;
; Every structural fact the Java patterns rest on is spelled differently here,
; which is precisely what makes this a falsification of "the interpreter is
; language-agnostic" rather than a restatement of it:
;
;   * Kotlin's `annotation` node carries NO `name:` field. An annotation with
;     arguments is a `constructor_invocation` — a `user_type` plus
;     `value_arguments` — and a marker annotation is a bare `user_type`. The
;     Java reader looks for `child_by_field_name("name")` and would find nothing
;     on either shape.
;   * There is no `element_value_pair`. `value_arguments` is a homogeneous list
;     of `value_argument` nodes, and a NAME is simply an `identifier` child in
;     front of the value — so an unanchored value pattern matches the named form
;     too, and every positional pattern below anchors with `.` for that reason
;     (the same trap `frameworks.scm` documents at length).
;   * There is no `field_declaration`. A property is a `class_parameter` of the
;     primary constructor, or a `property_declaration` in the body.
;
; ── The accessor convention differs too, and that IS descriptor data ─────────
;
; Kotlin reads a property by its own name (`config.uriGetArchive`) as well as
; through the JVM-interop getter a Java call site sees
; (`config.getUriGetArchive()`). Both spellings are `[properties]
; accessor_prefixes` rows in `plugin.toml` — `""` and `"get"`/`"is"` — so the
; convention is descriptor data here exactly as the vocabulary is. Nothing in
; this file knows about it.
;
; ── Patterns ─────────────────────────────────────────────────────────────────

; 1. The annotated declaration, in both annotation forms. `(user_type)`'s child
;    is the annotation's simple name; a qualified spelling nests further and is
;    the same stated ceiling Java records.
(class_declaration
  (modifiers
    (annotation [
      (constructor_invocation (user_type (identifier) @props.annotation))
      (user_type (identifier) @props.annotation)
    ]))
  name: (identifier) @props.class.name) @props.class

; 2. The NAMED prefix argument — `@ConfigurationProperties(prefix = "x.y")`.
;    `value` is Spring's alias for `prefix`; any other named argument binds
;    nothing. The annotation's own name is bound in the same match so the
;    interpreter can tell this class's key prefix from a sibling annotation's
;    argument — the rule is stated once, in `plugins/java/queries/properties.scm`
;    and in the interpreter's capture contract.
((class_declaration
  (modifiers
    (annotation
      (constructor_invocation
        (user_type (identifier) @props.annotation)
        (value_arguments
          (value_argument
            .
            (identifier) @_key
            (_) @props.prefix)))))) @props.class
 (#any-of? @_key "prefix" "value"))

; 3. The single-value form — `@ConfigurationProperties("x.y")`. Anchored to the
;    argument's FIRST named child, which is exactly the child a named argument's
;    key occupies, so it cannot also read a named argument's value.
(class_declaration
  (modifiers
    (annotation
      (constructor_invocation
        (user_type (identifier) @props.annotation)
        (value_arguments
          (value_argument
            .
            (string_literal) @props.prefix)))))) @props.class

; 4. Primary-constructor properties — the dominant `data class` shape.
;      @ConfigurationProperties(prefix = "mailserver.api")
;      data class MailServerApi(val uriGetArchive: String, val timeout: Int)
;    The parameter's name is its first named child; the type follows.
(class_declaration
  (primary_constructor
    (class_parameters
      (class_parameter
        .
        (identifier) @props.field)))) @props.class

; 5. Body properties — the `var`/`val` setter-bound shape.
(class_declaration
  (class_body
    (property_declaration
      (variable_declaration
        .
        (identifier) @props.field)))) @props.class

; ── Stated coverage ceilings (ADR-54: recorded, never worked around) ─────────
;
; NOT captured, beyond the two Java also records (a qualified annotation name, a
; prefix that is not a written literal):
;
;   * A declaration carrying TWO OR MORE annotations, when another top-level
;     declaration follows it in the file. `tree-sitter-kotlin-ng` 1.1 fails to
;     recognise it as a `class_declaration` at all — its modifiers are reparsed
;     as a file-level expression and `has_error()` stays false, so no pattern in
;     any query can see it. This is an upstream grammar defect, documented at
;     length in this plugin's `frameworks.scm`, not an exclusion chosen here.
;   * A `companion object`'s properties, and a destructured or delegated
;     property. Neither is a Spring-bound property shape.
;
; Like every capability query this file is droppable-on-disk: a copy at
; `.logos/plugins/kotlin/queries/properties.scm` shadows it without a rebuild
; (FR-PL-04, FR-PL-05).
