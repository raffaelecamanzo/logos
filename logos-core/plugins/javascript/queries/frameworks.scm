; JavaScript-only framework patterns (S-541, CR-174, FR-FW-03), layered over the
; shared TypeScript-family query: `grammars.rs` appends this file to
; `tsx/queries/frameworks.scm` for the `javascript` entry only, so TypeScript's
; routes are untouched. The pair compiles against the TSX Language and reads as
; one query; an on-disk `.logos/plugins/javascript/queries/frameworks.scm`
; replaces both (FR-PL-04).
;
; An Express path written as a template literal: `` app.get(`${base}/ping`, h) ``.
; The two shared Express patterns take a plain `(string)` path; these are the
; same two shapes over a `(template_string)`. Both carry the shared handler-less
; pattern's receiver rule, the handler-bearing one included: a template is how
; a client call spells an interpolated URL (`axios.get(`/api/items/${id}`,
; config)`, `https.get(`${base}/health`, onResponse)`), and unscoped, that call
; would be a route and its callback a fabricated handler (NFR-RA-05).
;
; Which templates are paths is the shared pass's rule, not this file's
; (`resolve::framework::is_a_url_path`, CR-110): the path is read verbatim, and
; kept when it carries a `/` or opens with a `${…}` reference. A template whose
; static tail starts with `/` is therefore a route, and its leading
; interpolation — a prefix this pass does not resolve — stays in the route's
; path (`GET ${base}/ping`) as a Spring `${api.base}` placeholder does (S-328);
; the shared route-template normalizer refuses to bind it.

((call_expression
  function: (member_expression
    object: [(identifier) (member_expression)] @fw.route.receiver
    property: (property_identifier) @fw.route.method)
  arguments: (arguments
    .
    (template_string) @fw.route.path
    .
    [(identifier) (member_expression)] @fw.route.handler))
  (#match? @fw.route.receiver "(^|\\.)([Aa]pp|[Rr]outer|[Ss]erver|[A-Za-z_$][A-Za-z0-9_$]*(Router|App|Server))$")
  (#not-match? @fw.route.method "^[A-Z]"))

((call_expression
  function: (member_expression
    object: [(identifier) (member_expression)] @fw.route.receiver
    property: (property_identifier) @fw.route.method)
  arguments: (arguments
    .
    (template_string) @fw.route.path))
  (#match? @fw.route.receiver "(^|\\.)([Aa]pp|[Rr]outer|[Ss]erver|[A-Za-z_$][A-Za-z0-9_$]*(Router|App|Server))$")
  (#not-match? @fw.route.method "^[A-Z]"))
