; Java test-quality smell-evidence query (CR-007, FR-CV-08; the optional 4th
; query). NOT a capability — `test_gaps` runs it on demand and post-filters the
; @smell.test candidates through the canonical test-marker logic (a @Test-family
; annotation). Droppable at `.logos/plugins/java/queries/smells.scm`.
;
; Capture contract — see governance::smells. Predicated patterns are wrapped in
; an explicit group `( … (#pred) )` so the predicate binds to the pattern.

; Candidate test methods: every method with a body.
(method_declaration
  name: (identifier) @smell.name
  body: (block) @smell.body) @smell.test

; Assertions: JUnit/AssertJ assertX(…) / fail(…), invoked by simple name
; (assertEquals, assertTrue, assertThat, fail, …).
(
  (method_invocation name: (identifier) @_a) @smell.assertion
  (#match? @_a "^(assert|fail)")
)

; Sleeping: Thread.sleep(…) — a `sleep(…)` invocation by simple name.
(
  (method_invocation name: (identifier) @_s) @smell.sleep
  (#eq? @_s "sleep")
)
