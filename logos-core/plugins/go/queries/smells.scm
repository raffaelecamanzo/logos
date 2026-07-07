; Go test-quality smell-evidence query (CR-007, FR-CV-08; the optional 4th
; query). NOT a capability — `test_gaps` runs it on demand and post-filters the
; @smell.test candidates through the canonical test-marker logic, which gates on
; a `*_test.go` filename plus a Test/Benchmark/Fuzz name (a gate a query cannot
; express). Droppable at `.logos/plugins/go/queries/smells.scm`.
;
; Capture contract — see governance::smells. Predicated patterns are wrapped in
; an explicit group `( … (#pred) )` so the predicate binds to the pattern.

; Candidate test functions: every top-level `func` with a body.
(function_declaration
  name: (identifier) @smell.name
  body: (block) @smell.body) @smell.test

; Assertions: t.Error/Errorf/Fatal/Fatalf/Fail/FailNow (the testing.T API), or a
; testify assert./require. helper (Equal/NoError/True/False/…).
(
  (call_expression
    function: (selector_expression field: (field_identifier) @_a)) @smell.assertion
  (#match? @_a "^(Errorf?|Fatalf?|Fail|FailNow|Equal|NotEqual|NoError|True|False|Nil|NotNil)$")
)

; Sleeping: time.Sleep(…).
(
  (call_expression
    function: (selector_expression field: (field_identifier) @_s)) @smell.sleep
  (#eq? @_s "Sleep")
)
