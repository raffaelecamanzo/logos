; Python test-quality smell-evidence query (CR-007, FR-CV-08; the optional 4th
; query). NOT a capability — `test_gaps` runs it on demand and post-filters the
; @smell.test candidates through the canonical test-marker logic (a `test_*`
; name, or a `test`-prefixed method of a TestCase subclass). Droppable at
; `.logos/plugins/python/queries/smells.scm` (FR-PL-04, FR-PL-05).
;
; Capture contract — see governance::smells. @smell.body is paired in-match with
; @smell.test; @smell.assertion / @smell.sleep are associated by containment.
; Predicated patterns are wrapped in an explicit group `( … (#pred) )`.

; Candidate test functions: every `def` with a body.
(function_definition
  name: (identifier) @smell.name
  body: (block) @smell.body) @smell.test

; Assertions: a bare `assert …` statement.
(assert_statement) @smell.assertion

; Assertions: a `self.assertEqual(…)` / `assertTrue(…)` unittest call, or a
; module-level `assertX(…)`.
(
  (call function: (attribute attribute: (identifier) @_a)) @smell.assertion
  (#match? @_a "^assert")
)
(
  (call function: (identifier) @_a2) @smell.assertion
  (#match? @_a2 "^assert")
)

; Sleeping: time.sleep(…) (any `.sleep(…)` attribute call).
(
  (call function: (attribute attribute: (identifier) @_s)) @smell.sleep
  (#eq? @_s "sleep")
)
