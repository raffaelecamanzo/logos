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
;   @ref.import.from — a MARKER, recording nothing on its own: the module of a
;                 `from m import a, b` (S-519, FR-RS-14). Each imported name
;                 is its own `@ref.import` match, recorded as its path under
;                 that module (`m::a`, `m::b`) — one row per imported name. A
;                 relative module keeps its level as leading `.`/`..` segments
;                 (`from .rules import Rule` → `.::rules::Rule`, `from ..x
;                 import y` → `..::x::y`), which the binder reads from the
;                 importing file's package. With `@ref.import.asterisk`
;                 (`from m import *`) the row is a glob of the module itself.
;   @ref.import.alias — the local name an `as` import binds (`import a as b`,
;                 `from m import a as b` → `b`; S-520). The row's alias is that
;                 name, never the imported one, so a call to `a` does not bind
;                 through an import that renamed it (S-519 review) and `b()`
;                 does. The path is still recorded under the imported module.
;
; Droppable on disk at `.logos/plugins/python/queries/references.scm`
; (FR-PL-04, FR-PL-05).

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
    name: (dotted_name) @ref.import
    alias: (identifier) @ref.import.alias))

(import_from_statement
  module_name: (_) @ref.import.from
  name: (dotted_name) @ref.import)

(import_from_statement
  module_name: (_) @ref.import.from
  name: (aliased_import
    name: (dotted_name) @ref.import
    alias: (identifier) @ref.import.alias))

(import_from_statement
  module_name: (_) @ref.import.from
  (wildcard_import) @ref.import @ref.import.asterisk)

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
