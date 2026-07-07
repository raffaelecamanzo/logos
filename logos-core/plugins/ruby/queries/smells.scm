; Ruby test-quality smell-evidence query (CR-007, FR-CV-08; the optional 4th
; query). NOT a capability — `test_gaps` runs it on demand and post-filters the
; @smell.test candidates through the canonical test-marker logic (a minitest
; `test_*` method, or an RSpec `it`/`describe` block). Droppable at
; `.logos/plugins/ruby/queries/smells.scm` (FR-PL-04, FR-PL-05).
;
; Capture contract — see governance::smells. @smell.body is paired in-match with
; @smell.test; @smell.assertion / @smell.sleep are associated by containment.
; Predicated patterns are wrapped in an explicit group `( … (#pred) )`.

; Candidate minitest test units: every method with a body (the `test_*` filter
; is applied by the canonical post-filter, not here).
(method
  name: (identifier) @smell.name
  body: (body_statement) @smell.body) @smell.test

; Candidate RSpec examples: `it "…" do … end` (and the specify/example/scenario
; aliases). The description string is the display name; the `do_block` body is
; paired for empty-body detection (an empty `do … end` has no body_statement).
((call
  method: (identifier) @_ex
  arguments: (argument_list (string) @smell.name)
  block: (do_block) @smell.body) @smell.test
  (#any-of? @_ex "it" "specify" "example" "scenario"))

; Assertions: a minitest `assert*`/`refute*` call, or an RSpec `expect(...)`.
(
  (call !receiver method: (identifier) @_a) @smell.assertion
  (#match? @_a "^(assert|refute|expect)")
)

; Sleeping: a bare `sleep(…)` call.
(
  (call !receiver method: (identifier) @_s) @smell.sleep
  (#eq? @_s "sleep")
)
