; PHP reference-extraction query (S-060, capability = "references").
;
;   @ref.call   — a free function call's name (`callee()`); name-only,
;                 policy-gated binding.
;   @ref.method — an instance (`$svc->list()`) or static (`Foo::bar()`) method
;                 call name; name-only, bound by its receiver's SHAPE (the
;                 markers below).
;   @ref.import — a `use` clause's namespace path (`Illuminate\Support\…\Route`);
;                 canonicalised (backslash → `::`) into the ledger form feeding
;                 the binder and the framework candidacy gate (FR-FW-04).
;   @ref.import.alias — the local name a `use A\B as C;` binds (`C`; S-520). The
;                 row's alias is that name, not `B`; without an `as` it stays
;                 the path's last segment.
;   @ref.access — an own-property access (`$this->balance`): a method reading a
;                 field of its own class (CR-005, FR-EX-08), the bound LCOM4 input.
;   @ref.extends — a class's parent (`extends Base`), an interface's parents:
;                 `Extends` (class → class, interface → interface; S-522,
;                 FR-RS-15).
;   @ref.implements — a class's or enum's interfaces (`implements I`), and the
;                 traits a class, trait or enum `use`s (`use Loggable;`):
;                 `Implements` (→ interface or trait). Both bind through the
;                 declared namespace's rungs — the file's `use` imports, its own
;                 namespace, then the fully-qualified name. A name written with
;                 a leading `\` is fully qualified, read from the global
;                 namespace alone.
;
; Droppable on disk at `.logos/plugins/php/queries/references.scm`.
;
; Deliberately NOT captured in v1: `new Class()` construction references (no
; constructor symbol to bind them — see symbols.scm), dynamic calls
; (`$obj->$method()`, `call_user_func`), variable-variable indirection — they
; stay honestly unresolved (NFR-RA-05, never fabricate).

(function_call_expression
  function: (name) @ref.call)

(member_call_expression
  name: (name) @ref.method)

(scoped_call_expression
  name: (name) @ref.method)

;   Receiver markers (S-515, CR-169, FR-EX-13) — MARKERS, recording nothing on
;   their own. Each names the receiver (`object`) or scope (`scope`) of the
;   method call above: its captured node shares the call expression the
;   `@ref.method` name sits in, and the call records that SHAPE, which the
;   binder dispatches on (FR-RS-12).
;
;   @ref.receiver.self      — `$this->m()`, `self::m()`, `static::m()`: binds
;                             among the enclosing class's own members only.
;   @ref.receiver.super     — `parent::m()`: binds only through a proven
;                             `Extends` of the caller's class (`@ref.extends`
;                             below) — the nearest parent holding exactly one
;                             `m` — and never reaches the caller's own class.
;   @ref.receiver.other     — every receiver or scope (`$svc->m()`, `Foo::m()`,
;                             `$this->repo->m()`): never binds. `self`/`super`
;                             outrank it.
;   @ref.receiver.anonymous — an anonymous class's body (`new class { … }`): it
;                             has no class node, so a `$this->m()` inside is
;                             never the enclosing class's `m` — `other`, unless
;                             the body declares `m` itself.
;
;   An unqualified call (`m()`) is never a call on the instance in PHP: it is
;   the free `@ref.call` above (`implicit_receiver` stays "none").
((member_call_expression object: (variable_name) @ref.receiver.self)
  (#eq? @ref.receiver.self "$this"))
((scoped_call_expression scope: (relative_scope) @ref.receiver.self)
  (#any-of? @ref.receiver.self "self" "static"))
((scoped_call_expression scope: (relative_scope) @ref.receiver.super)
  (#eq? @ref.receiver.super "parent"))
(member_call_expression object: (_) @ref.receiver.other)
(scoped_call_expression scope: (_) @ref.receiver.other)
(anonymous_class body: (declaration_list) @ref.receiver.anonymous)

(namespace_use_clause
  (qualified_name) @ref.import
  alias: (name)? @ref.import.alias)

;   @ref.access — `$this->field`: an own-field read/write. `member_access_expression`
; is a distinct node from `member_call_expression`, so this never double-captures
; a method call. The class lexically Contains both the method and its
; `property_declaration` fields, so resolution binds an exactly-one Field
; candidate to an `Accesses` edge (Method → Field); an ambiguous access stays
; unresolved (NFR-RA-05).
((member_access_expression
  object: (variable_name (name) @_recv)
  name: (name) @ref.access)
  (#eq? @_recv "this"))

;   @ref.extends / @ref.implements — anchored to the declaration that owns the
;   clause, so an anonymous class's (`new class extends Base {}`) is never read
;   as the enclosing function's: it has no class node to relate.
(class_declaration
  (base_clause [(name) (qualified_name) (relative_name)] @ref.extends))
(interface_declaration
  (base_clause [(name) (qualified_name) (relative_name)] @ref.extends))
(class_declaration
  (class_interface_clause [(name) (qualified_name) (relative_name)] @ref.implements))
(enum_declaration
  (class_interface_clause [(name) (qualified_name) (relative_name)] @ref.implements))
(class_declaration
  body: (declaration_list
    (use_declaration [(name) (qualified_name) (relative_name)] @ref.implements)))
(trait_declaration
  body: (declaration_list
    (use_declaration [(name) (qualified_name) (relative_name)] @ref.implements)))
(enum_declaration
  body: (enum_declaration_list
    (use_declaration [(name) (qualified_name) (relative_name)] @ref.implements)))
