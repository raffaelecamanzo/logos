; C reference-extraction query (S-056, capability = "references").
;
;   @ref.call   — a direct function call (`f()`), recorded in Path form: bound
;                 to the caller's own file's definition, else one the files
;                 its includes reach supply, else the one non-`static`
;                 definition of the name among the repository's C files —
;                 the link unit (S-584, FR-RS-40; `plugin.toml`). Two
;                 candidates at a step bind nothing (NFR-RA-05).
;   @ref.import — an `#include` header (S-584): `"x.h"` read against the
;                 including file's directory, then the include roots; `<a/b.h>`
;                 against the include roots alone (`plugin.toml`), as C++
;                 reads its own (S-579). A header no root holds (`<stdio.h>`)
;                 stays external. Its row binds the included file and names
;                 nothing, so it carries no alias.
;
; Droppable on disk at `.logos/plugins/c/queries/references.scm`. Deliberately
; NOT captured: an `#include` a macro names (`#include HEADER`) — no
; preprocessor runs — and indirect calls through function pointers (no
; statically bindable callee — never fabricated).
(call_expression
  function: (identifier) @ref.call)

; `#include "x.h"` / `#include <a/b.h>` (S-584). A macro-named header is
; neither node kind, so it records nothing.
(preproc_include
  path: [(string_literal) (system_lib_string)] @ref.import)

; ── Argument count (S-591, CR-190, FR-EX-32) ─────────────────────────────────
; The `@arity.*` vocabulary the extraction engine reads (`extract::arity`): every
; argument list, whose named children a call row counts, and the forms that
; make a count unknown. Captures record no row of their own, so every ledger
; target is unchanged.
(argument_list) @arity.arguments
