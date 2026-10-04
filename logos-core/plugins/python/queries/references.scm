; Python reference-extraction query (S-015, capability = "references").
;
; The capture name carries the reference shape (see extract::collect_refs):
;   @ref.call   — a plain-identifier call (`f()`); the text is the path.
;   @ref.method — an attribute call (`obj.m()`, `module.f()`); only the
;                 attribute name is knowable without type inference, so it
;                 binds by its receiver's SHAPE (the markers below).
;   @ref.import — an import path; the captured node's *text* is canonicalised
;                 (dots → `::`) into the ledger form that feeds both the
;                 binder and the framework candidacy gate (FR-FW-04).
;
; Droppable on disk at `.logos/plugins/python/queries/references.scm`
; (FR-PL-04, FR-PL-05).
;
; Deliberately NOT captured in v1 (documented limitations, S-015):
;   - the imported *names* of `from m import a, b` (only the module path is
;     recorded; per-name aliases are a later increment);
;   - `import x as y` rename binding (the path is recorded, the `as` name is
;     not yet an alias).

(call
  function: (identifier) @ref.call)

(call
  function: (attribute
    attribute: (identifier) @ref.method))

;   Receiver markers (S-515, CR-169, FR-EX-13) — MARKERS, recording nothing on
;   their own. Each names the receiver of the attribute call above: its
;   captured node shares the `attribute` the `@ref.method` name sits in, and the
;   call records that SHAPE, which the binder dispatches on (FR-RS-12).
;
;   @ref.receiver.self  — `self.m()`, `cls.m()`: the conventional instance and
;                         class parameters; binds among the enclosing class's
;                         own members only.
;   @ref.receiver.super — `super().m()`, `super(A, self).m()`: the inner `super`
;                         call is the receiver. Binds only through a proven
;                         `Extends`, which this plugin does not record — so it
;                         stays unbound, and never reaches the caller's class.
;   @ref.receiver.other — every receiver (`obj.m()`, `module.f()`,
;                         `self.x.m()`): never binds. `self`/`super` outrank it.
;
;   An unqualified call (`m()`) is never a call on the instance in Python: it
;   is the free `@ref.call` above (`implicit_receiver` stays "none").
((call
  function: (attribute object: (identifier) @ref.receiver.self))
  (#any-of? @ref.receiver.self "self" "cls"))
((call
  function: (attribute
    object: (call function: (identifier) @_super) @ref.receiver.super))
  (#eq? @_super "super"))
(call
  function: (attribute object: (_) @ref.receiver.other))

(import_statement
  name: (dotted_name) @ref.import)

(import_statement
  name: (aliased_import
    name: (dotted_name) @ref.import))

(import_from_statement
  module_name: (dotted_name) @ref.import)

(import_from_statement
  module_name: (relative_import) @ref.import)

;   @ref.access — an own-attribute access (`self.x`): a method reading an
;                 attribute of its own class (CR-005, FR-EX-08). The `#eq? self`
;                 predicate anchors the capture to the conventional receiver, so
;                 only own-member accesses are recorded. Resolution binds it to
;                 an `Accesses` edge only on an exactly-one Field candidate, else
;                 it stays unresolved (NFR-RA-05).
((attribute
  object: (identifier) @_recv
  attribute: (identifier) @ref.access)
  (#eq? @_recv "self"))
