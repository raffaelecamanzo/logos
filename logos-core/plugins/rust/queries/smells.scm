; Rust test-quality smell-evidence query (CR-007, FR-CV-08; the optional 4th
; query). NOT a declared capability — extraction never runs it; `test_gaps`
; runs it on demand over the current tree and post-filters the @smell.test
; candidates through the canonical test-marker logic (so a #[test]/#[cfg(test)]
; gate, not the query, decides what is a test). Droppable on disk at
; `.logos/plugins/rust/queries/smells.scm` — shadowing it changes detection
; with no rebuild (FR-PL-04, FR-PL-05, UAT-CV-04).
;
; Capture contract (read by governance::smells, language-agnostic):
;   @smell.test  — a candidate test function (its body + name paired in-match)
;   @smell.body  — that function's body block (empty-body detection)
;   @smell.name  — the function's name (the finding label)
;   @smell.assertion — an assertion site (associated to a test by containment)
;   @smell.sleep — a real-time delay site (associated by containment)
;
; Predicated patterns are wrapped in an explicit group `( … (#pred) )` so the
; predicate binds to the pattern (the canonical tree-sitter form).

; Candidate test functions: every `fn` with a body. test_evidence gates which
; are actually tests (#[test]-family attribute or a #[cfg(test)] module).
(function_item
  name: (identifier) @smell.name
  body: (block) @smell.body) @smell.test

; Assertions: assert!/assert_eq!/assert_ne!/debug_assert*!/panic! macros.
(
  (macro_invocation macro: (identifier) @_a) @smell.assertion
  (#match? @_a "^(assert|debug_assert|panic)")
)

; Sleeping: thread::sleep(…) / std::thread::sleep(…) / a bare sleep(…).
(
  (call_expression
    function: [
      (identifier) @_s
      (scoped_identifier name: (identifier) @_s)
    ]) @smell.sleep
  (#eq? @_s "sleep")
)
