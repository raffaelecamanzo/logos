; Scala test-quality smell-evidence query (CR-007, FR-CV-08; the optional 4th
; query). NOT a capability — `test_gaps` runs it on demand and post-filters the
; @smell.test candidates through the canonical test-marker logic (the munit /
; ScalaTest `test`/`it` marker call). Droppable at
; `.logos/plugins/scala/queries/smells.scm`.
;
; Capture contract — see governance::smells. Predicated patterns are wrapped in
; an explicit group `( … (#pred) )` so the predicate binds to the pattern.

; Candidate test units: a `test("name") { … }` / `it("name") { … }` call. The
; munit/ScalaTest form is a curried call — the outer call's `function` is the
; inner `test("name")` call and its `arguments` is the block body. @smell.name
; captures the description string (quote-stripped by governance); @smell.test is
; the whole call (the node the test-marker logic confirms); @smell.body is the
; block (for empty-body detection).
(
  (call_expression
    function: (call_expression
      function: (identifier) @_marker
      arguments: (arguments (string) @smell.name))
    arguments: (block) @smell.body) @smell.test
  (#match? @_marker "^(test|it)$")
)

; Assertions: munit/ScalaTest assertX(…) / fail(…) invoked by simple name
; (assert, assertEquals, assertTrue, assertResult, fail, …).
(
  (call_expression function: (identifier) @_a) @smell.assertion
  (#match? @_a "^(assert|fail)")
)

; Sleeping: Thread.sleep(…) — a `sleep(…)` member call by simple name.
(
  (call_expression
    function: (field_expression field: (identifier) @_s)) @smell.sleep
  (#eq? @_s "sleep")
)
