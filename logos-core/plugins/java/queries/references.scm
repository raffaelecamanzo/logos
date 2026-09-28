; Java reference-extraction query (S-015, capability = "references").
;
;   @ref.call        — a receiver-less invocation's name (`list()`): the scope
;                      hierarchy binds it — the enclosing class, then a static
;                      import naming it (CR-149) — as every other language's
;                      plain call is (Go, Python, PHP, Ruby, C).
;   @ref.method      — a receiver invocation's name (`service.list()`,
;                      `List.of()`); name-only, policy-gated binding (receiver
;                      typing is a resolution concern, CR-150). A receiver call
;                      never takes its target from the file's imports, so the
;                      two shapes must stay apart: recorded alike, `List.of()`
;                      bound to a statically imported in-house `of`.
;   @ref.import      — an import declaration's scoped path
;                      (`org.springframework.web…`, `static a.b.C.m`);
;                      canonicalised (dots → `::`) into the ledger form feeding
;                      the binder and the framework candidacy gate (FR-FW-04).
;   @ref.import.static — the `static` keyword, present in the same match for a
;                      static import; with the wildcard marker it makes the glob
;                      a static one (every static member of the type, not just
;                      its member types).
;   @ref.import.asterisk — present in the same match when the declaration ends
;                      in `.*` (CR-149): the path before it (`a.b.*` → `a::b`,
;                      `static a.b.C.*` → `a::b::C`) is then recorded as a glob —
;                      it names a package or a type whose members it brings into
;                      scope, never one declaration.
;
; One pattern, with the wildcard as an optional marker rather than a second
; pattern kept apart by a last-child anchor: a comment is a named node too, so
; `import a.b.C /* why */;` would slip past such an anchor and record nothing.
;
; Droppable on disk at `.logos/plugins/java/queries/references.scm`.
;
; Deliberately NOT captured in v1: `new T()` construction references (no
; constructor nodes exist to bind them to — see symbols.scm).

(method_invocation
  !object
  name: (identifier) @ref.call)

(method_invocation
  object: (_)
  name: (identifier) @ref.method)

(import_declaration
  "static"? @ref.import.static
  (scoped_identifier) @ref.import
  (asterisk)? @ref.import.asterisk)

;   @ref.access — an own-field access (`this.x`): a method reading a field of
;                 its own class (CR-005, FR-EX-08). `field_access` is a distinct
;                 node from `method_invocation`, so this never double-captures a
;                 method call. The class lexically Contains both the method and
;                 its `field` declarations (symbols.scm captures Java fields), so
;                 resolution binds an exactly-one Field candidate to an
;                 `Accesses` edge (Method → Field); an ambiguous access stays
;                 unresolved (NFR-RA-05). This is the bound LCOM4 input.
(field_access
  object: (this)
  field: (identifier) @ref.access)
