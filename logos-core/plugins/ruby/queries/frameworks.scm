; Ruby framework-extraction query (S-059, capability = "frameworks") — the
; ratified set: Rails (FR-FW-03).
;
; Declarative capture contract (resolve::framework::generic_match): a pattern
; captures the registration parts directly —
;   @fw.route.path    — the URL string literal (unquoted by the pass);
;   @fw.route.method  — the node whose text maps through [framework_methods]
;                       (unmapped text drops the match, FR-FW-04 best-effort);
;   @fw.component.name — a component declaration's name;
;   @fw.component.base — predicate-only helper, not consumed by the pass.
;
; Droppable on disk at `.logos/plugins/ruby/queries/frameworks.scm`.
;
; Deliberately NOT captured: the RESTful `resources`/`resource` macros (each
; expands to up to seven routes — statically expanding them would fabricate,
; NFR-RA-05) and `match … via:`.

; ── Where a route is a route: inside `routes.draw` (S-567) ──────────────────────
;
; A Rails route is a verb call inside the block `Rails.application.routes.draw`
; takes. The same call anywhere else is something else — rack's test client
; writes `get("/cgi/test", …)` — so every registration below is marked
; `@fw.route.nested`: a route only inside a nesting scope, dropped silently
; outside every one (`resolve::framework::nest`). The `draw` block is that
; scope, with the root `/` as its prefix.
;
; Rails' `normalize_path` drops a trailing slash, so a route written `/` inside
; a prefixing block is the block's prefix itself — `scope '/admin' do get '/' …
; end` serves `/admin`, not `/admin/` — which every nesting block below states
; with `@fw.route.prefix.absorbs`. It sits on the method name, not on the block:
; tree-sitter keeps at most three captures per pattern node, and a prefixing
; block already carries three.
((call
   receiver: (call method: (identifier) @fw.route.prefix.key)
   method: (identifier) @fw.route.prefix.name @fw.route.prefix.root @fw.route.prefix.absorbs
   block: [(do_block) (block)] @fw.route.prefix.scope @fw.route.prefix.nest)
  (#eq? @fw.route.prefix.key "routes")
  (#eq? @fw.route.prefix.name "draw"))

; ── Prefixes: `namespace` and `scope` ────────────────────────────────────────────
;
; Every nesting scope around a registration applies, outermost first:
; `namespace :mod do get "x" … end` serves `/mod/x`. A namespace's path is its
; name — a symbol (`@fw.route.prefix.symbol`, read without its colon) or a
; string — unless a `path:` option names another; a scope's path is its first
; positional argument or its `path:` option. A path the file cannot read (a
; constant, an interpolated string) is opaque, and its registrations are
; refused and counted rather than promoted at a partial path (FR-FW-05).
; Every one of these blocks — and the `resources` blocks below — is marked
; `@fw.route.prefix.within`: it joins its prefix onto the `draw` chain but is no
; router by itself, so a `namespace` outside `routes.draw` (a routes file another
; one `draw`s, a Grape API, a test) establishes no route.
((call
   !receiver
   method: (identifier) @fw.route.prefix.name @fw.route.prefix.absorbs
   arguments: (argument_list . (simple_symbol) @fw.route.prefix.symbol) @fw.route.prefix.key
   block: [(do_block) (block)] @fw.route.prefix.scope @fw.route.prefix.nest @fw.route.prefix.within)
  (#any-of? @fw.route.prefix.name "namespace" "scope")
  (#not-match? @fw.route.prefix.key "(^|[^A-Za-z0-9_:])path:|:path\\s*=>"))

((call
   !receiver
   method: (identifier) @fw.route.prefix.name @fw.route.prefix.absorbs
   arguments: (argument_list . (string) @fw.route.prefix) @fw.route.prefix.key
   block: [(do_block) (block)] @fw.route.prefix.scope @fw.route.prefix.nest @fw.route.prefix.within)
  (#any-of? @fw.route.prefix.name "namespace" "scope")
  (#not-match? @fw.route.prefix.key "(^|[^A-Za-z0-9_:])path:|:path\\s*=>"))

; A `path:` option: a written string is the path, anything else is opaque (an
; opaque capture exactly at a captured literal is that literal, restated).
((call
   !receiver
   method: (identifier) @fw.route.prefix.name @fw.route.prefix.absorbs
   arguments: (argument_list
     (pair
       key: [(hash_key_symbol) (simple_symbol)] @fw.route.prefix.value
       value: (string) @fw.route.prefix))
   block: [(do_block) (block)] @fw.route.prefix.scope @fw.route.prefix.nest @fw.route.prefix.within)
  (#any-of? @fw.route.prefix.name "namespace" "scope")
  (#any-of? @fw.route.prefix.value "path" ":path"))

((call
   !receiver
   method: (identifier) @fw.route.prefix.name @fw.route.prefix.absorbs
   arguments: (argument_list
     (pair
       key: [(hash_key_symbol) (simple_symbol)] @fw.route.prefix.value
       value: (_) @fw.route.prefix.opaque))
   block: [(do_block) (block)] @fw.route.prefix.scope @fw.route.prefix.nest @fw.route.prefix.within)
  (#any-of? @fw.route.prefix.name "namespace" "scope")
  (#any-of? @fw.route.prefix.value "path" ":path"))

; A namespace or scope whose first argument is anything but an option names a
; path this file cannot read — a constant, `ENV["P"] || "/"`, a `:"a/b"` symbol.
; Every first argument is captured opaque and only an option pair (`path: …`,
; `:module => …`) is excluded, so an argument kind no pattern names fails CLOSED;
; a symbol or string this file reads is captured above at exactly this range and
; is that literal, restated.
((call
   !receiver
   method: (identifier) @fw.route.prefix.name @fw.route.prefix.absorbs
   arguments: (argument_list . (_) @fw.route.prefix.opaque)
   block: [(do_block) (block)] @fw.route.prefix.scope @fw.route.prefix.nest @fw.route.prefix.within)
  (#any-of? @fw.route.prefix.name "namespace" "scope")
  (#not-match? @fw.route.prefix.opaque "^([A-Za-z_][A-Za-z0-9_]*:\\s|:[A-Za-z_][A-Za-z0-9_]*\\s*=>|\"[^\"]*\"\\s*=>|'[^']*'\\s*=>)"))

; `resources`, `resource` and `concern` stay unexpanded: a registration in their
; block sits under the resource's member or collection path, which is not
; written, so the block is a nesting scope whose prefix cannot be read — its
; registrations are refused and counted, never promoted at a guessed address.
((call
   !receiver
   method: (identifier) @fw.route.prefix.name @fw.route.prefix.absorbs
   arguments: (argument_list) @fw.route.prefix.opaque
   block: [(do_block) (block)] @fw.route.prefix.scope @fw.route.prefix.nest @fw.route.prefix.within)
  (#any-of? @fw.route.prefix.name "resources" "resource" "concern"))

; ── Modules: what a `controller#action` handler is qualified by ─────────────────
;
; A namespace puts its controllers in the module its name spells
; (`namespace :mod` → `Mod`), unless a `module:` option names another; a
; scope's `module:` option does the same, and a scope without one adds no
; module. A `module:` the file cannot read leaves every handler inside naming
; nothing (`@fw.route.module.opaque`).
((call
   !receiver
   method: (identifier) @fw.route.module.name
   arguments: (argument_list . [(simple_symbol) (string)] @fw.route.module) @fw.route.module.args
   block: [(do_block) (block)] @fw.route.module.scope)
  (#eq? @fw.route.module.name "namespace")
  (#not-match? @fw.route.module.args "(^|[^A-Za-z0-9_:])module:|:module\\s*=>"))

; A namespace whose name is not a plain symbol or string spells a module this
; file cannot read, and its handlers name nothing.
((call
   !receiver
   method: (identifier) @fw.route.module.name
   arguments: (argument_list . (_) @fw.route.module.value)
   block: [(do_block) (block)] @fw.route.module.scope @fw.route.module.opaque)
  (#eq? @fw.route.module.name "namespace")
  (#not-match? @fw.route.module.value "^(:[A-Za-z_][A-Za-z0-9_]*|\"[^\"#]*\"|'[^']*')$"))

((call
   !receiver
   method: (identifier) @fw.route.module.name
   arguments: (argument_list
     (pair
       key: [(hash_key_symbol) (simple_symbol)] @fw.route.module.key
       value: [(simple_symbol) (string)] @fw.route.module))
   block: [(do_block) (block)] @fw.route.module.scope)
  (#any-of? @fw.route.module.name "namespace" "scope")
  (#any-of? @fw.route.module.key "module" ":module"))

((call
   !receiver
   method: (identifier) @fw.route.module.name
   arguments: (argument_list
     (pair
       key: [(hash_key_symbol) (simple_symbol)] @fw.route.module.key
       value: (_) @fw.route.module.value))
   block: [(do_block) (block)] @fw.route.module.scope @fw.route.module.opaque)
  (#any-of? @fw.route.module.name "namespace" "scope")
  (#any-of? @fw.route.module.key "module" ":module")
  (#not-match? @fw.route.module.value "^(:[A-Za-z_][A-Za-z0-9_]*|\"[^\"#]*\"|'[^']*')$"))

; ── The registrations ─────────────────────────────────────────────────────────
;
; The verb maps through `[framework_methods]`. A handler written as a
; `controller#action` string (`@fw.route.handler.action`) names the instance
; method `action` of the class its controller name spells with the suffix this
; framework appends (`#set! fw.route.handler.suffix`): `"stories#show"` is
; `StoriesController#show`, inside the modules around it. A `redirect(…)` or any
; other handler names nothing, and the route keeps its node with no edge.
;
; `get "/x" => "c#a"` — the hash-rocket form, lobsters' dominant shape.
((call
   !receiver
   method: (identifier) @fw.route.method @fw.route.nested
   arguments: (argument_list
     . (pair key: (string) @fw.route.path value: (string) @fw.route.handler.action))) @fw.route.anchor
  (#set! fw.route.handler.suffix "Controller"))

((call
   !receiver
   method: (identifier) @fw.route.method @fw.route.nested
   arguments: (argument_list . (pair key: (string) @fw.route.path))) @fw.route.anchor)

; `get "/x", to: "c#a"` (or `:to => "c#a"`), the option anywhere among the
; options.
((call
   !receiver
   method: (identifier) @fw.route.method @fw.route.nested
   arguments: (argument_list
     . (string) @fw.route.path
     (pair
       key: [(hash_key_symbol) (simple_symbol)] @fw.route.key
       value: (string) @fw.route.handler.action))) @fw.route.anchor
  (#any-of? @fw.route.key "to" ":to")
  (#not-eq? @fw.route.method "root")
  (#set! fw.route.handler.suffix "Controller"))

; A path and no handler string: `get "/x", to: redirect(…)`, `delete "/x"`.
; `root` writes no path, so its positional string is never one.
((call
   !receiver
   method: (identifier) @fw.route.method @fw.route.nested
   arguments: (argument_list . (string) @fw.route.path)) @fw.route.anchor
  (#not-eq? @fw.route.method "root"))

; `root to: "c#a"` and `root "c#a"`: the route at the enclosing path itself.
((call
   !receiver
   method: (identifier) @fw.route.method @fw.route.nested
   arguments: (argument_list
     (pair
       key: [(hash_key_symbol) (simple_symbol)] @fw.route.key
       value: (string) @fw.route.handler.action))) @fw.route.anchor
  (#eq? @fw.route.method "root")
  (#any-of? @fw.route.key "to" ":to")
  (#set! fw.route.handler.suffix "Controller"))

((call
   !receiver
   method: (identifier) @fw.route.method @fw.route.nested
   arguments: (argument_list . (string) @fw.route.handler.action)) @fw.route.anchor
  (#eq? @fw.route.method "root")
  (#set! fw.route.handler.suffix "Controller"))

((call
   !receiver
   method: (identifier) @fw.route.method @fw.route.nested
   arguments: (argument_list)) @fw.route.anchor
  (#eq? @fw.route.method "root"))

; A Rails controller or model: a class whose superclass names a Rails base
; (`ApplicationController`/`…Controller`, `ApplicationRecord`/`…Record`,
; `ActiveRecord::Base`) is the wired application building block (FR-FW-02).
((class
  name: (constant) @fw.component.name
  superclass: (superclass
    [(constant) (scope_resolution)] @fw.component.base))
  (#match? @fw.component.base "(Controller|Record|Base)$"))
