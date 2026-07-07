; Scala reference-extraction query (S-061, CR-009, capability = "references").
;
;   @ref.method — a method/function invocation's name, by simple name
;                 (`compute()`) or as the selected member of a receiver
;                 (`helper.doThing()`). Name-only, policy-gated binding (receiver
;                 typing is a resolution concern), yielding a `Calls` edge.
;   @ref.access — an own-field access (`this.x`): a method reading a field of
;                 its own class (CR-005, FR-EX-08), the bound LCOM4 input. The
;                 class lexically Contains both the method and its `val`/`var`
;                 members, so resolution binds an exactly-one Field candidate to
;                 an `Accesses` edge; an ambiguous/unmatched access stays
;                 unresolved (NFR-RA-05).
;
; Droppable on disk at `.logos/plugins/scala/queries/references.scm`.
;
; Deliberately NOT captured in v1 (best-effort, never fabricated): import edges
; — Scala flattens a dotted `import a.b.c` into repeated `path:` identifiers with
; no single spanning node, and selector/wildcard imports (`import a.{b, c}`,
; `import a.*`) need a structural walk beyond a text split (the same reason Rust
; keeps a dedicated `ref.use` walk). Their absence lowers measured cross-file
; resolution coverage but never produces a wrong edge.

; A call by simple name: `compute()`, `assert(...)`, the `test`/`it` markers.
(call_expression
  function: (identifier) @ref.method)

; A call selecting a member of a receiver: `helper.doThing()` — the method name.
(call_expression
  function: (field_expression
    field: (identifier) @ref.method))

; An own-field access `this.x` (the bound cohesion input). Over-capturing a
; `this.m()` method call here is harmless: with no Field named `m` in the class,
; the Accesses candidate simply never binds (NFR-RA-05).
(
  (field_expression
    value: (identifier) @_recv
    field: (identifier) @ref.access)
  (#eq? @_recv "this")
)
