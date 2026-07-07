; C++ test-quality smell-evidence query (CR-007, FR-CV-08; the optional 4th
; query). NOT a capability — `test_gaps` runs it on demand and post-filters the
; @smell.test candidates through the canonical test-marker logic (the
; cpp-test-macros convention: a GoogleTest `TEST`/`TEST_F`/… function). Droppable
; at `.logos/plugins/cpp/queries/smells.scm`.
;
; Capture contract — see governance::smells. Predicated patterns are wrapped in
; an explicit group `( … (#pred) )` so the predicate binds to the pattern.

; Candidate test units: every free function definition with a body. The
; post-filter keeps only those the cpp-test-macros convention confirms (a
; `TEST`/`TEST_F` macro), so the broad capture never over-reports. `@smell.name`
; is the declarator identifier — the macro keyword for a GoogleTest test, which
; is exactly what `test_evidence` matches.
(function_definition
  declarator: (function_declarator
    declarator: (identifier) @smell.name)
  body: (compound_statement) @smell.body) @smell.test

; Assertions: the GoogleTest (`EXPECT_*`/`ASSERT_*`) and Catch2
; (`REQUIRE`/`CHECK`/`REQUIRE_*`) assertion macros, invoked by simple name.
(
  (call_expression function: (identifier) @_a) @smell.assertion
  (#match? @_a "^(EXPECT_|ASSERT_|REQUIRE|CHECK|SUCCEED|FAIL|ADD_FAILURE)")
)

; Sleeping: a bare `sleep`/`usleep`/`nanosleep` call …
(
  (call_expression function: (identifier) @_s) @smell.sleep
  (#match? @_s "^(sleep|usleep|nanosleep)$")
)

; … or a qualified `…::sleep_for`/`…::sleep_until` (`std::this_thread::sleep_for`):
; the innermost `name: (identifier)` is the simple name, and the captured
; `qualified_identifier` lies inside the call, so its range is contained by the
; test unit.
(
  (qualified_identifier name: (identifier) @_s2) @smell.sleep
  (#match? @_s2 "^(sleep_for|sleep_until)$")
)
