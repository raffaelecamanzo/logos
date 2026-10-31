; PHP framework-extraction query (S-060, capability = "frameworks") — the
; ratified set: Laravel and Symfony (FR-FW-03, S-549).
;
; Declarative capture contract (resolve::framework::generic_match): see the
; Python query's header for the capture vocabulary. Droppable on disk at
; `.logos/plugins/php/queries/frameworks.scm`.
;
; The framework pass only runs on files whose ledger names the `Illuminate` or
; `Symfony` canonical prefix (FR-FW-04 candidacy gate, set in plugin.toml
; `framework_detectors`), so a `Route::` static call, a `#[Route]` attribute or
; a `Model` base in a file that imports neither framework never promotes
; anything.
;
; Both frameworks serve every path from the root whether or not it was written
; with a leading slash (Laravel's `Route::get('me', …)` is `/me`), so every
; route and prefix pattern carries `@fw.route.rooted` — that is what lets a
; Laravel route match an OpenAPI `/api/me`, and what keeps a single-segment path
; (`me`, `ping`) a path rather than a property key (`is_a_url_path`).
;
; A literal is a single- or a double-quoted string — `string` and
; `encapsed_string` are different node kinds, so every literal position names
; both. A double-quoted string that interpolates (`"v{$n}"`, `"users/$id"`) is not
; a literal: as a prefix the shared resolvability rule refuses it (with the routes
; under it), and a path pattern rejects it with a `#not-match?` on `$` or `\`.
;
; Deliberately NOT captured: Laravel route handlers (passed as
; `[Controller::class, 'method']` arrays or `'Controller@method'` strings, not
; plain provable names — the route node is promoted with no fabricated RoutesTo
; edge, NFR-RA-05); `Route::resource`/`Route::apiResource` (not expanded into
; the routes they imply, CR-176 §3.3: neither verb is in `[framework_methods]`,
; so they promote nothing); `Route::match(…)` (its first argument is the verb
; list or a verb string, not a path, and `match` maps to no verb); registrar chains ending in a verb
; (`Route::middleware('x')->get(…)`); a group that loads a route file instead of
; a closure; Symfony's localized path maps (`path: ['en' => '/about']`) and
; docblock `@Route` annotations.

; ── Laravel ──────────────────────────────────────────────────────────────────

; Laravel route facade: `Route::get('/users', …)` — the verb maps through
; [framework_methods] (an unmapped verb promotes nothing); the scope must be the
; `Route` facade. The first argument is the URL path string (unquoted by the pass).
((scoped_call_expression
  scope: (name) @_scope @fw.route.rooted
  name: (name) @fw.route.method
  arguments: (arguments
    .
    (argument [(string) (encapsed_string)] @fw.route.path)))
  (#eq? @_scope "Route")
  (#not-match? @fw.route.path "[$\\\\]"))

; Laravel route group prefix: `Route::prefix('api')->group(function () { … })`.
; The governed range is the group's closure, so every route registered inside it
; takes the prefix. Laravel groups NEST — a group inside a prefixed group
; appends its prefix to the outer one (`api` + `os/s3` → `/api/os/s3`), which is
; what `@fw.route.prefix.nested` asks of the shared interpreter (Spring's
; innermost-prefix rule is the default it overrides). Laravel trims both sides
; of the join, so a route written `/` in a group IS the group's prefix
; (`Route::prefix('users')` + `Route::get('/')` serves `/users`, not `/users/`):
; `@fw.route.prefix.absorbs`, on the closure (tree-sitter keeps at most three
; captures per pattern node, and the `group` name carries three). Symfony's
; class `#[Route]` does not carry it — its `#[Route('/')]` method under
; `#[Route('/blog')]` really serves `/blog/`. A group with no prefix
; (`Route::middleware('auth')->group(…)`) declares no scope at all, so the routes
; in it keep the prefix of the group around it.
;
; `prefix()` may sit anywhere in the registrar chain before `group()` —
; `Route::prefix('api')->middleware('api')->group(…)` and
; `Route::middleware('os.auth')->prefix('os/s3')->group(…)` are both koel's. A
; query cannot descend a chain of unbounded length, so one pattern per distance
; is written: `prefix()` directly before `group()`, or one, two or three calls
; earlier. A longer chain reads no prefix.
;
; The argument is either a literal (`@fw.route.prefix`) or anything else
; (`@fw.route.prefix.opaque` — a variable, a `config()` call, an interpolated
; string): an opaque prefix refuses the routes under it rather than promoting
; them at a partial address (FR-FW-05). The opaque alternative matches the
; literal too, at an identical range, which the interpreter ignores.
((member_call_expression
  object: [
    (scoped_call_expression
      name: (name) @_prefix
      arguments: (arguments . (argument [(string) @fw.route.prefix (encapsed_string) @fw.route.prefix (_) @fw.route.prefix.opaque])))
    (member_call_expression
      name: (name) @_prefix
      arguments: (arguments . (argument [(string) @fw.route.prefix (encapsed_string) @fw.route.prefix (_) @fw.route.prefix.opaque])))
  ]
  name: (name) @_group @fw.route.rooted @fw.route.prefix.nested
  arguments: (arguments
    (argument [(anonymous_function) (arrow_function)] @fw.route.prefix.scope @fw.route.prefix.absorbs)))
  (#eq? @_prefix "prefix")
  (#eq? @_group "group"))

((member_call_expression
  object: (member_call_expression
    object: [
      (scoped_call_expression
        name: (name) @_prefix
        arguments: (arguments . (argument [(string) @fw.route.prefix (encapsed_string) @fw.route.prefix (_) @fw.route.prefix.opaque])))
      (member_call_expression
        name: (name) @_prefix
        arguments: (arguments . (argument [(string) @fw.route.prefix (encapsed_string) @fw.route.prefix (_) @fw.route.prefix.opaque])))
    ])
  name: (name) @_group @fw.route.rooted @fw.route.prefix.nested
  arguments: (arguments
    (argument [(anonymous_function) (arrow_function)] @fw.route.prefix.scope @fw.route.prefix.absorbs)))
  (#eq? @_prefix "prefix")
  (#eq? @_group "group"))

((member_call_expression
  object: (member_call_expression
    object: (member_call_expression
      object: [
        (scoped_call_expression
          name: (name) @_prefix
          arguments: (arguments . (argument [(string) @fw.route.prefix (encapsed_string) @fw.route.prefix (_) @fw.route.prefix.opaque])))
        (member_call_expression
          name: (name) @_prefix
          arguments: (arguments . (argument [(string) @fw.route.prefix (encapsed_string) @fw.route.prefix (_) @fw.route.prefix.opaque])))
      ]))
  name: (name) @_group @fw.route.rooted @fw.route.prefix.nested
  arguments: (arguments
    (argument [(anonymous_function) (arrow_function)] @fw.route.prefix.scope @fw.route.prefix.absorbs)))
  (#eq? @_prefix "prefix")
  (#eq? @_group "group"))

((member_call_expression
  object: (member_call_expression
    object: (member_call_expression
      object: (member_call_expression
        object: [
          (scoped_call_expression
            name: (name) @_prefix
            arguments: (arguments . (argument [(string) @fw.route.prefix (encapsed_string) @fw.route.prefix (_) @fw.route.prefix.opaque])))
          (member_call_expression
            name: (name) @_prefix
            arguments: (arguments . (argument [(string) @fw.route.prefix (encapsed_string) @fw.route.prefix (_) @fw.route.prefix.opaque])))
        ])))
  name: (name) @_group @fw.route.rooted @fw.route.prefix.nested
  arguments: (arguments
    (argument [(anonymous_function) (arrow_function)] @fw.route.prefix.scope @fw.route.prefix.absorbs)))
  (#eq? @_prefix "prefix")
  (#eq? @_group "group"))

; The array form: `Route::group(['prefix' => 'radio', 'middleware' => …], function () { … })`.
; Only the `prefix` key is a prefix; the other attributes are not paths.
((scoped_call_expression
  scope: (name) @_scope
  name: (name) @_group @fw.route.rooted @fw.route.prefix.nested
  arguments: (arguments
    .
    (argument
      (array_creation_expression
        (array_element_initializer
          .
          [(string (string_content) @_key) (encapsed_string (string_content) @_key)]
          .
          [(string) @fw.route.prefix (encapsed_string) @fw.route.prefix (_) @fw.route.prefix.opaque])))
    (argument [(anonymous_function) (arrow_function)] @fw.route.prefix.scope @fw.route.prefix.absorbs)))
  (#eq? @_scope "Route")
  (#eq? @_group "group")
  (#eq? @_key "prefix"))

; Laravel / Symfony building block (FR-FW-02): an Eloquent model (`class X
; extends Model`) or a controller (`extends Controller`, Symfony's
; `extends AbstractController`) — the base name (or its last namespace segment)
; ends in `Model`/`Controller`. `@fw.component.base` exists only for the
; predicate.
((class_declaration
  name: (name) @fw.component.name
  (base_clause [(name) (qualified_name)] @fw.component.base))
  (#match? @fw.component.base "(Model|Controller)$"))

; ── Symfony ──────────────────────────────────────────────────────────────────
;
; `#[Route('/blog')] class BlogController { #[Route('/{slug}', methods: ['GET'])]
; public function show() {} }`. Composition is the shared interpreter's, exactly
; as for Spring: this file CAPTURES the class prefix and the method path and never
; joins them (`resolve::framework::compose_prefixes`). The attribute is pinned to
; the bare name `Route` (the `use …\Attribute\Route;` spelling); an aliased or
; fully-qualified attribute name is not read.
;
; The verb: each string of `methods:` (a list or a single string), or Symfony's
; own `Request::METHOD_*` constant (`@_request` pins the class), maps through
; [framework_methods]. A route with no `methods:` argument, or an empty
; `methods: []`, answers every verb, which is the attribute name itself mapped to
; ANY — pinned to that case by the `#not-match?` on the argument text, which
; rejects only a non-empty `methods:`. Any other verb expression (a variable,
; another class's constant) names no verb this pass can prove and promotes
; nothing.
;
; A method path that is not a written literal (`#[Route(self::PATH)]`) is captured
; as `@fw.route.path.opaque` / `@fw.route.path.named.opaque`: this dialect folds no
; constants, so it is refused and counted, never promoted. Each path position is
; one alternation of the literal and the opaque form; the opaque form matches a
; literal too, at an identical range, which the interpreter skips.

; The class prefix: positional (`#[Route('/blog')]`) or named (`path: '/blog'`).
; An opaque prefix (`#[Route(self::BASE)]`) refuses the routes under it.
((class_declaration
  attributes: (attribute_list
    (attribute_group
      (attribute
        (name) @fw.route.prefix.name @fw.route.rooted
        parameters: (arguments
          .
          (argument !name [(string) @fw.route.prefix (encapsed_string) @fw.route.prefix (_) @fw.route.prefix.opaque]))))))
  @fw.route.prefix.scope
  (#eq? @fw.route.prefix.name "Route"))

((class_declaration
  attributes: (attribute_list
    (attribute_group
      (attribute
        (name) @fw.route.prefix.name @fw.route.rooted
        parameters: (arguments
          (argument
            name: (name) @fw.route.prefix.key
            .
            [(string) @fw.route.prefix (encapsed_string) @fw.route.prefix (_) @fw.route.prefix.opaque]))))))
  @fw.route.prefix.scope
  (#eq? @fw.route.prefix.name "Route")
  (#eq? @fw.route.prefix.key "path"))

; The method route, positional path, no `methods:` — every verb.
((method_declaration
  attributes: (attribute_list
    (attribute_group
      (attribute
        (name) @fw.route.method @fw.route.rooted
        parameters: (arguments
          .
          (argument !name [(string) @fw.route.path (encapsed_string) @fw.route.path (_) @fw.route.path.opaque])) @fw.route.args) @fw.route.anchor))
  name: (name) @fw.route.handler)
  (#eq? @fw.route.method "Route")
  (#not-match? @fw.route.path "[$\\\\]")
  (#not-match? @fw.route.args "methods[ \t\r\n]*:[ \t\r\n]*(\\[[ \t\r\n]*[^\\] \t\r\n]|[^\\[ \t\r\n])"))

; …named `path:`, no `methods:`.
((method_declaration
  attributes: (attribute_list
    (attribute_group
      (attribute
        (name) @fw.route.method @fw.route.rooted
        parameters: (arguments
          (argument
            name: (name) @fw.route.key
            .
            [(string) @fw.route.path.named (encapsed_string) @fw.route.path.named (_) @fw.route.path.named.opaque])) @fw.route.args) @fw.route.anchor))
  name: (name) @fw.route.handler)
  (#eq? @fw.route.method "Route")
  (#eq? @fw.route.key "path")
  (#not-match? @fw.route.path.named "[$\\\\]")
  (#not-match? @fw.route.args "methods[ \t\r\n]*:[ \t\r\n]*(\\[[ \t\r\n]*[^\\] \t\r\n]|[^\\[ \t\r\n])"))

; …positional path with `methods:` — one route per listed verb.
((method_declaration
  attributes: (attribute_list
    (attribute_group
      (attribute
        (name) @_attr @fw.route.rooted
        parameters: (arguments
          .
          (argument !name [(string) @fw.route.path (encapsed_string) @fw.route.path (_) @fw.route.path.opaque])
          (argument
            name: (name) @_methods
            .
            [
              (array_creation_expression
                (array_element_initializer
                  [
                    (string (string_content) @fw.route.method)
                    (encapsed_string (string_content) @fw.route.method)
                    (class_constant_access_expression . (name) @_request . (name) @fw.route.method)
                  ]))
              (string (string_content) @fw.route.method)
              (encapsed_string (string_content) @fw.route.method)
              (class_constant_access_expression . (name) @_request . (name) @fw.route.method)
            ]))) @fw.route.anchor))
  name: (name) @fw.route.handler)
  (#eq? @_attr "Route")
  (#not-match? @fw.route.path "[$\\\\]")
  (#eq? @_methods "methods")
  (#eq? @_request "Request"))

; …named `path:` with `methods:`, in either order (named arguments may be written
; in any order).
((method_declaration
  attributes: (attribute_list
    (attribute_group
      (attribute
        (name) @_attr @fw.route.rooted
        parameters: (arguments
          (argument
            name: (name) @fw.route.key
            .
            [(string) @fw.route.path.named (encapsed_string) @fw.route.path.named (_) @fw.route.path.named.opaque])
          (argument
            name: (name) @_methods
            .
            [
              (array_creation_expression
                (array_element_initializer
                  [
                    (string (string_content) @fw.route.method)
                    (encapsed_string (string_content) @fw.route.method)
                    (class_constant_access_expression . (name) @_request . (name) @fw.route.method)
                  ]))
              (string (string_content) @fw.route.method)
              (encapsed_string (string_content) @fw.route.method)
              (class_constant_access_expression . (name) @_request . (name) @fw.route.method)
            ]))) @fw.route.anchor))
  name: (name) @fw.route.handler)
  (#eq? @_attr "Route")
  (#not-match? @fw.route.path.named "[$\\\\]")
  (#eq? @fw.route.key "path")
  (#eq? @_methods "methods")
  (#eq? @_request "Request"))

((method_declaration
  attributes: (attribute_list
    (attribute_group
      (attribute
        (name) @_attr @fw.route.rooted
        parameters: (arguments
          (argument
            name: (name) @_methods
            .
            [
              (array_creation_expression
                (array_element_initializer
                  [
                    (string (string_content) @fw.route.method)
                    (encapsed_string (string_content) @fw.route.method)
                    (class_constant_access_expression . (name) @_request . (name) @fw.route.method)
                  ]))
              (string (string_content) @fw.route.method)
              (encapsed_string (string_content) @fw.route.method)
              (class_constant_access_expression . (name) @_request . (name) @fw.route.method)
            ])
          (argument
            name: (name) @fw.route.key
            .
            [(string) @fw.route.path.named (encapsed_string) @fw.route.path.named (_) @fw.route.path.named.opaque]))) @fw.route.anchor))
  name: (name) @fw.route.handler)
  (#eq? @_attr "Route")
  (#not-match? @fw.route.path.named "[$\\\\]")
  (#eq? @fw.route.key "path")
  (#eq? @_methods "methods")
  (#eq? @_request "Request"))
