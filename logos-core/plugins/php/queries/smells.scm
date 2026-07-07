; PHP test-quality smell-evidence query (CR-007, FR-CV-08; the optional 4th
; query). NOT a capability — `test_gaps` runs it on demand and post-filters the
; @smell.test candidates through the canonical test-marker logic (the PHPUnit
; convention: a `test*` name, a `#[Test]` attribute, or a `@test` docblock).
; Droppable at `.logos/plugins/php/queries/smells.scm`.
;
; Capture contract — see governance::smells. Predicated patterns are wrapped in
; an explicit group `( … (#pred) )` so the predicate binds to the pattern.

; Candidate test methods: every method with a body.
(method_declaration
  name: (name) @smell.name
  body: (compound_statement) @smell.body) @smell.test

; Assertions: PHPUnit `$this->assertEquals(…)` (member), `self::assertTrue(…)`
; (static), the `assert*(…)`/`expect*(…)` functional helpers, and `fail(…)`,
; all invoked by simple name.
(
  (member_call_expression name: (name) @_a) @smell.assertion
  (#match? @_a "^(assert|expect|fail)")
)
(
  (scoped_call_expression name: (name) @_sa) @smell.assertion
  (#match? @_sa "^(assert|expect|fail)")
)
(
  (function_call_expression function: (name) @_fa) @smell.assertion
  (#match? @_fa "^(assert|expect|fail)")
)

; Sleeping: `sleep(…)` / `usleep(…)` — a function call by simple name.
(
  (function_call_expression function: (name) @_s) @smell.sleep
  (#match? @_s "^u?sleep$")
)
