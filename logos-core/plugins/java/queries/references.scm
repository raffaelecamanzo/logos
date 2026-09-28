; Java reference-extraction query (S-015, capability = "references").
;
;   @ref.method      — a method invocation's name (`service.list()`, `list()`);
;                      name-only, policy-gated binding (receiver typing is a
;                      resolution concern).
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
