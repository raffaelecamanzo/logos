; JavaScript-only symbol patterns (S-539, CR-174, FR-EX-18), layered over the
; shared TypeScript-family query: `grammars.rs` appends this file to
; `tsx/queries/symbols.scm` for the `javascript` entry only, so TypeScript's
; symbols are untouched. The pair compiles against the TSX Language and reads
; as one query; an on-disk `.logos/plugins/javascript/queries/symbols.scm`
; replaces both (FR-PL-04).
;
; A function defined by assignment is a declaration on its assignment (or its
; object-literal `pair`), named by the property it assigns:
;
; - `X.y = function … | (…) => …` is a Function `y` of the function or class
;   named `X`, or the top-level `const`/`let`/`var`, that the assignment sees —
;   read from the function it is written in outwards — or a module-level
;   Function when it sees none (`exports.y`, a mixin's parameter, a nested
;   local, `window.y`). `module.exports = …` is the module's own export,
;   not a function of `module`, and `this.y = …` is a field (S-542, below).
; - `module.exports.y = …` is a module-level Function. Both `exports` forms are
;   exported (`shape::is_exported`).
; - `Foo.prototype.y = …` is a Method of the declaration named `Foo`, or a
;   module-level Method when the file holds none.
; - `{ y: function … | (…) => … }` is a Method of the declaration enclosing the
;   object — a `const` it is bound to included — or a module-level Method.
; - `this.y = …` in a class's constructor or method is a Field `y` of that class
;   (S-542, `@symbol.attribute`, as Python's `self.y = …`): taken at its name,
;   so the assignment's value stays the method's, and declared once per class —
;   a class field written `y = 0;` or an earlier write is the declaration, and
;   the rest declare nothing. Not captured: `this.#y` (a `#private` field is
;   declared in the class body), `this.y += …`, a destructuring target, and a
;   `this.y = …` outside a method of a class (a constructor function's, an
;   object literal's) — no class owns it.
;
; Only a function or arrow value declares a function: `X.y = 5`, `{ n: 1 }` and a
; computed or quoted key declare none. Each function is placed by `extract::owner` (the
; `@owner.*` vocabulary) and owns no scope, so a declaration written inside its
; body keeps the symbol it had before S-539.

(assignment_expression
  left: (member_expression
    object: (identifier) @owner.name
    property: (property_identifier) @symbol.function)
  right: [(function_expression
            parameters: (formal_parameters) @arity.parameters)
          (arrow_function
            parameters: (formal_parameters) @arity.parameters)
          (arrow_function
            parameter: (identifier) @arity.parameters @arity.required)]
  (#not-eq? @owner.name "module"))

(assignment_expression
  left: (member_expression
    object: (member_expression
      object: (identifier) @_module
      property: (property_identifier) @_exports)
    property: (property_identifier) @symbol.function)
  right: [(function_expression
            parameters: (formal_parameters) @arity.parameters)
          (arrow_function
            parameters: (formal_parameters) @arity.parameters)
          (arrow_function
            parameter: (identifier) @arity.parameters @arity.required)]
  (#eq? @_module "module")
  (#eq? @_exports "exports")) @owner.file

(assignment_expression
  left: (member_expression
    object: (member_expression
      object: (identifier) @owner.name
      property: (property_identifier) @_prototype)
    property: (property_identifier) @symbol.method)
  right: [(function_expression
            parameters: (formal_parameters) @arity.parameters)
          (arrow_function
            parameters: (formal_parameters) @arity.parameters)
          (arrow_function
            parameter: (identifier) @arity.parameters @arity.required)]
  (#eq? @_prototype "prototype"))

(pair
  key: (property_identifier) @symbol.method
  value: [(function_expression
            parameters: (formal_parameters) @arity.parameters)
          (arrow_function
            parameters: (formal_parameters) @arity.parameters)
          (arrow_function
            parameter: (identifier) @arity.parameters @arity.required)]) @owner.enclosing

(assignment_expression
  left: (member_expression
    object: (this)
    property: (property_identifier) @symbol.field @symbol.attribute))
