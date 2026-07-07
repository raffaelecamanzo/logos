; Kotlin symbol-extraction query (S-055, capability = "symbols").
;
; Capture names map to NodeKind via NodeKind::as_str (extract engine).
; Compiled against the built Kotlin Language at load; fails fast naming this
; file on drift (FR-PL-02). Droppable on disk at
; `.logos/plugins/kotlin/queries/symbols.scm` (FR-PL-04, UAT-PL-03).
;
; class-bearing applicability (FR-QM-11): mapping a class construct to
; `@symbol.class` (NodeKind::Class) is the whole declaration — "the kind a
; construct extracts to is its declared applicability" (metrics::extended). So
; Cohesion/Focus apply to Kotlin classes (and enum classes / objects) and the
; field-sharing LCOM4 premise holds.
;
; v1 policy: Kotlin's `class_declaration` covers `class`, `interface`, and
; `enum class`; the anonymous `class`/`interface` keyword token disjointly
; partitions the two so no declaration is double-captured. An `enum class`
; carries the `class` token (plus an `enum` modifier), so it folds into Class —
; class-like and applicability-bearing, which is the honest answer for LCOM4.

; A class (or enum class) → Class.
(class_declaration
  "class"
  name: (identifier) @symbol.class)

; An interface (incl. `fun interface`) → Interface.
(class_declaration
  "interface"
  name: (identifier) @symbol.interface)

; An `object` singleton → Class (a class-like stateful container).
(object_declaration
  name: (identifier) @symbol.class)

; Every `fun` (top-level or member) → Function, the uniform v1 policy Rust also
; uses (its `impl` methods collapse to Function): the kind is fixed by the
; capture name, and Kotlin models member and free functions with one node, so a
; single mapping keeps extraction honest rather than guessing nesting.
(function_declaration
  name: (identifier) @symbol.function)

; A class property → Field — the bound LCOM4 field-sharing input read by
; `this.<name>` accesses (references.scm). Scoped to `class_body` so only true
; member properties are captured: Kotlin reuses `property_declaration` for
; function-local `val`/`var` and top-level properties too, which are NOT fields
; and must not pollute the graph (the same boundary Java's class-only
; `field_declaration` gets for free). `class_body` is shared by `class`,
; `object`, and companion objects, so their properties are all covered; only
; `enum class` bodies (rarely property-bearing) fall outside, an accepted gap.
(class_body
  (property_declaration
    (variable_declaration
      (identifier) @symbol.field)))
