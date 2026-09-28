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
;   @ref.import      — a single-type or single-static import's scoped path
;                      (`org.springframework.web…`, `static a.b.C.m`);
;                      canonicalised (dots → `::`) into the ledger form feeding
;                      the binder and the framework candidacy gate (FR-FW-04).
;   @ref.import.glob — a wildcard import's scoped path, the name before `.*`
;                      (`a.b.*` → `a::b`, `static a.b.C.*` → `a::b::C`),
;                      recorded as a glob (CR-149): it names a package or a type
;                      whose members it brings into scope, never one declaration.
;
; The two import patterns partition `import_declaration`: the trailing `.`
; anchor makes `@ref.import` match only when the path is the declaration's LAST
; named child, which a wildcard's `asterisk` child never lets it be — so no
; import is captured twice.
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
  (scoped_identifier) @ref.import .)

(import_declaration
  (scoped_identifier) @ref.import.glob
  .
  (asterisk))

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
