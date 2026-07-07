; Kotlin test-quality smell-evidence query (CR-007, FR-CV-08; the optional 4th
; query). NOT a capability — `test_gaps` runs it on demand and post-filters the
; @smell.test candidates through the canonical test-marker logic (a @Test-family
; annotation, the `kotlin-annotations` convention). Droppable at
; `.logos/plugins/kotlin/queries/smells.scm`.
;
; Capture contract — see governance::smells. Predicated patterns are wrapped in
; an explicit group `( … (#pred) )` so the predicate binds to the pattern.

; Candidate test functions: every function with a block body. `@smell.body` is
; the inner `block` (not the `function_body` wrapper) so empty-body detection
; reads its named children directly.
(function_declaration
  name: (identifier) @smell.name
  (function_body
    (block) @smell.body)) @smell.test

; Assertions: JUnit/kotlin.test assertX(…) / fail(…), invoked by simple name
; (assertEquals, assertTrue, assertFailsWith, fail, …).
(
  (call_expression (identifier) @_a) @smell.assertion
  (#match? @_a "^(assert|fail)")
)

; Sleeping: `Thread.sleep(…)` — a `sleep` member call via navigation. The
; predicate keeps the `sleep` member identifier (the receiver identifier is
; filtered out), so the enclosing call counts once.
(
  (call_expression
    (navigation_expression (identifier) @_s)) @smell.sleep
  (#eq? @_s "sleep")
)
