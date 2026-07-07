; C# test-quality smell-evidence query (CR-007, CR-009, FR-CV-08; the optional
; 4th query). NOT a capability — `test_gaps` runs it on demand and post-filters
; the @smell.test candidates through the canonical test-marker logic (a
; `[Fact]`/`[Theory]`/`[Test]`/`[TestMethod]` attribute). Droppable at
; `.logos/plugins/c-sharp/queries/smells.scm`.
;
; Capture contract — see governance::smells. Predicated patterns are wrapped in
; an explicit group `( … (#pred) )` so the predicate binds to the pattern.

; Candidate test methods: every method with a block body.
(method_declaration
  name: (identifier) @smell.name
  body: (block) @smell.body) @smell.test

; Assertions: xUnit/NUnit/MSTest `Assert.X(…)` (and the NUnit constraint/typed
; assert helpers), invoked on a recognised assertion class.
(
  (invocation_expression
    function: (member_access_expression
      expression: (identifier) @_recv)) @smell.assertion
  (#any-of? @_recv "Assert" "ClassicAssert" "StringAssert" "CollectionAssert")
)

; Sleeping: `Thread.Sleep(…)` — a `.Sleep(…)` member invocation.
(
  (invocation_expression
    function: (member_access_expression
      name: (identifier) @_s)) @smell.sleep
  (#eq? @_s "Sleep")
)
