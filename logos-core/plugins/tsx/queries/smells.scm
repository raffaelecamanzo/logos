; TSX (TypeScript + JSX) test-quality smell-evidence query (CR-007, FR-CV-08;
; the optional 4th query). NOT a capability — `test_gaps` runs it on demand and
; post-filters @smell.test candidates through the canonical test-marker logic
; (a function lexically enclosed by an it/test call). Unlike the other languages
; the unit is the callback itself, so the query pre-scopes @smell.test to the
; arrow/function that is the *direct* callback argument to `it(…)`/`test(…)` —
; a nested helper is not a test. Droppable at
; `.logos/plugins/tsx/queries/smells.scm`.
;
; Capture contract — see governance::smells. @smell.name is the test description
; string; @smell.body is paired in-match with @smell.test. Predicated patterns
; are wrapped in an explicit group `( … (#pred) )`.

; Candidate test units: the callback of `it('desc', () => {…})` / `test(…)`,
; including `it.only`/`test.each` member forms.
(
  (call_expression
    function: [
      (identifier) @_fn
      (member_expression object: (identifier) @_fn)
    ]
    arguments: (arguments
      [(string) (template_string)] @smell.name
      [
        (arrow_function body: (statement_block) @smell.body)
        (function_expression body: (statement_block) @smell.body)
      ] @smell.test))
  (#any-of? @_fn "it" "test")
)

; Assertions: expect(…) (incl. the head of an expect(x).toBe(y) chain) and
; assert(…).
(
  (call_expression function: (identifier) @_a) @smell.assertion
  (#any-of? @_a "expect" "assert")
)

; Assertions: assert.equal(…) / chai.* member-style assertions.
(
  (call_expression
    function: (member_expression object: (identifier) @_ao)) @smell.assertion
  (#any-of? @_ao "assert" "chai")
)

; Sleeping: setTimeout(…) / a bare sleep(…) / delay(…) real-time wait.
(
  (call_expression function: (identifier) @_s) @smell.sleep
  (#any-of? @_s "setTimeout" "sleep" "delay")
)
