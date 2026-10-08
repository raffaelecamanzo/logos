/*
 * The widget source scan (S-617, CR-203 §6, FR-UI-39, FR-UI-40). It closes the
 * claim "every web widget" with a read of the views' SOURCE, not a hand-kept
 * list: the S-438 `?raw` precedent (`views/dashboard/cardEnumeration.ts`),
 * parsed here with the TypeScript compiler rather than regular expressions, so a
 * multi-line tag, an aliased import or a spread of props reads as what it is.
 *
 * Over each view source it finds:
 *   - every `Card` the view renders directly — a problem, since a view renders
 *     its widgets through `Widget` (which composes `Card` itself);
 *   - every `Widget`, and what it names: a catalogue entry (its `copy`, through
 *     an import from a `*.copy.ts` module, a local alias or a spread of props) or
 *     a registered tool panel (its `panel` key). A widget that names neither is a
 *     problem.
 *
 * It is a pure function of the sources it is handed and the catalogues it is
 * told exist: the test (`views/widgetScan.test.ts`) supplies both, from
 * `import.meta.glob`, and states the count as a finding, never a floor.
 */

import * as ts from "typescript";

/** What a catalogue module exports, by name: one entry, or a record of entries
 *  (e.g. one entry per Health dimension, indexed by the view). */
export type CatalogueExport = "entry" | "record";

/** Catalogue module path ("/src/copy/health.copy.ts") → its exports. */
export type CatalogueIndex = ReadonlyMap<string, ReadonlyMap<string, CatalogueExport>>;

export interface WidgetUse {
  readonly file: string;
  readonly line: number;
  /** The title as written: a literal, or `(dynamic)`. */
  readonly title: string;
  /** What it names: `"/src/copy/x.copy.ts#entry"`, or `"panel:key"`. */
  readonly names: string;
}

export interface ScanProblem {
  readonly file: string;
  readonly line: number;
  readonly problem: string;
}

export interface ScanReport {
  readonly widgets: readonly WidgetUse[];
  /** The view files that render at least one widget. */
  readonly views: readonly string[];
  readonly problems: readonly ScanProblem[];
  /** Every tool-panel key a widget names. */
  readonly panelsUsed: ReadonlySet<string>;
}

/** `spec` resolved against the directory of `from` (both "/src/…" paths). */
export function resolveImport(from: string, spec: string): string {
  if (!spec.startsWith(".")) return spec;
  const parts = from.split("/").slice(0, -1);
  for (const seg of spec.split("/")) {
    if (seg === "..") parts.pop();
    else if (seg !== ".") parts.push(seg);
  }
  return parts.join("/");
}

interface Imported {
  readonly module: string;
  readonly exported: string;
}

/** The local names this file binds to catalogue exports, and to the design
 *  system's `Card` and `Widget`. */
function readImports(file: string, sf: ts.SourceFile) {
  const catalogue = new Map<string, Imported>();
  const cards = new Set<string>();
  const widgets = new Set<string>();
  for (const stmt of sf.statements) {
    if (!ts.isImportDeclaration(stmt) || !ts.isStringLiteral(stmt.moduleSpecifier)) continue;
    const module = resolveImport(file, stmt.moduleSpecifier.text);
    const named = stmt.importClause?.namedBindings;
    if (!named || !ts.isNamedImports(named)) continue;
    for (const el of named.elements) {
      const exported = (el.propertyName ?? el.name).text;
      const local = el.name.text;
      if (module.endsWith(".copy.ts")) catalogue.set(local, { module, exported });
      if (/\/components(\/index\.ts|\/Card\.tsx|\/Widget\.tsx)?$/.test(module)) {
        if (exported === "Card") cards.add(local);
        if (exported === "Widget") widgets.add(local);
      }
    }
  }
  return { catalogue, cards, widgets };
}

/** The initializer of the nearest `const|let name = …` visible from `from`:
 *  the enclosing blocks first, innermost out, then the file. */
function findDeclaration(name: string, from: ts.Node): ts.Expression | undefined {
  for (let node: ts.Node | undefined = from; node; node = node.parent) {
    const statements = ts.isBlock(node) || ts.isSourceFile(node) ? node.statements : undefined;
    if (!statements) continue;
    for (const stmt of statements) {
      if (!ts.isVariableStatement(stmt)) continue;
      for (const decl of stmt.declarationList.declarations) {
        if (ts.isIdentifier(decl.name) && decl.name.text === name && decl.initializer) return decl.initializer;
      }
    }
  }
  return undefined;
}

/** `x as const`, `x satisfies T`, `(x)` → `x`. */
function unwrap(expr: ts.Expression): ts.Expression {
  while (ts.isAsExpression(expr) || ts.isSatisfiesExpression(expr) || ts.isParenthesizedExpression(expr)) {
    expr = expr.expression;
  }
  return expr;
}

const lineOf = (sf: ts.SourceFile, node: ts.Node) => sf.getLineAndCharacterOfPosition(node.getStart(sf)).line + 1;

const tagName = (node: ts.JsxOpeningElement | ts.JsxSelfClosingElement) =>
  ts.isIdentifier(node.tagName) ? node.tagName.text : node.tagName.getText();

/**
 * Scans the given view sources (path → text). `catalogues` lists every
 * catalogue module and its exports; `panels` every registered tool-panel key.
 */
export function scanViewSources(
  sources: Readonly<Record<string, string>>,
  catalogues: CatalogueIndex,
  panels: ReadonlySet<string>,
): ScanReport {
  const widgets: WidgetUse[] = [];
  const problems: ScanProblem[] = [];
  const panelsUsed = new Set<string>();

  for (const [file, text] of Object.entries(sources)) {
    const sf = ts.createSourceFile(file, text, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
    const imports = readImports(file, sf);
    const problem = (node: ts.Node, what: string) => problems.push({ file, line: lineOf(sf, node), problem: what });

    /** What a `copy` expression names, or why it names nothing. */
    const resolveCopy = (expr: ts.Expression, depth = 0): { names: string } | { problem: string } => {
      expr = unwrap(expr);
      if (depth > 4) return { problem: "copy resolves through too many aliases to read" };
      if (ts.isConditionalExpression(expr)) {
        const a = resolveCopy(expr.whenTrue, depth + 1);
        if ("problem" in a) return a;
        const b = resolveCopy(expr.whenFalse, depth + 1);
        return "problem" in b ? b : { names: `${a.names} | ${b.names}` };
      }
      // `RECORD[key]` / `RECORD.key`: the base must be a record of entries.
      if (ts.isElementAccessExpression(expr) || ts.isPropertyAccessExpression(expr)) {
        const base = unwrap(expr.expression);
        if (!ts.isIdentifier(base)) return { problem: `copy={${expr.getText(sf)}} does not name a catalogue` };
        const imported = imports.catalogue.get(base.text);
        if (!imported) return { problem: `copy={${expr.getText(sf)}}: ${base.text} is not imported from a catalogue` };
        const kind = catalogues.get(imported.module)?.get(imported.exported);
        if (kind !== "record") {
          return { problem: `copy={${expr.getText(sf)}}: ${imported.exported} is not a record of catalogue entries in ${imported.module}` };
        }
        return { names: `${imported.module}#${imported.exported}[…]` };
      }
      if (ts.isIdentifier(expr)) {
        const imported = imports.catalogue.get(expr.text);
        if (imported) {
          const kind = catalogues.get(imported.module)?.get(imported.exported);
          if (kind !== "entry") {
            return { problem: `copy={${expr.text}}: ${imported.module} has no catalogue entry ${imported.exported}` };
          }
          return { names: `${imported.module}#${imported.exported}` };
        }
        const local = findDeclaration(expr.text, expr);
        if (local) return resolveCopy(local, depth + 1);
        return { problem: `copy={${expr.text}} is neither imported from a catalogue nor declared here` };
      }
      return { problem: `copy={${expr.getText(sf)}} does not name a catalogue entry` };
    };

    /** The `copy` property of a spread object (`{...common}`), if it has one. */
    const copyOfSpread = (expr: ts.Expression): ts.Expression | undefined => {
      let obj = unwrap(expr);
      if (ts.isIdentifier(obj)) {
        const init = findDeclaration(obj.text, obj);
        if (!init) return undefined;
        obj = unwrap(init);
      }
      if (!ts.isObjectLiteralExpression(obj)) return undefined;
      for (const prop of obj.properties) {
        if (ts.isPropertyAssignment(prop) && prop.name.getText(sf) === "copy") return prop.initializer;
        if (ts.isShorthandPropertyAssignment(prop) && prop.name.text === "copy") return prop.name;
      }
      return undefined;
    };

    const visitWidget = (node: ts.JsxOpeningElement | ts.JsxSelfClosingElement) => {
      let title = "(dynamic)";
      let copy: ts.Expression | undefined;
      let panel: ts.Node | undefined;
      for (const attr of node.attributes.properties) {
        if (ts.isJsxSpreadAttribute(attr)) {
          copy ??= copyOfSpread(attr.expression);
          continue;
        }
        const name = attr.name.getText(sf);
        const init = attr.initializer;
        if (name === "title" && init && ts.isStringLiteral(init)) title = init.text;
        if (name === "copy" && init && ts.isJsxExpression(init) && init.expression) copy = init.expression;
        if (name === "panel") panel = init ?? attr;
      }
      const line = lineOf(sf, node);
      if (panel) {
        const key =
          ts.isStringLiteral(panel)
            ? panel.text
            : ts.isJsxExpression(panel) && panel.expression && ts.isStringLiteral(unwrap(panel.expression))
              ? (unwrap(panel.expression) as ts.StringLiteral).text
              : undefined;
        if (key === undefined) return problem(node, `<Widget title="${title}">: panel is not a literal key`);
        if (!panels.has(key)) return problem(node, `<Widget title="${title}">: panel "${key}" is not registered in TOOL_PANELS`);
        panelsUsed.add(key);
        widgets.push({ file, line, title, names: `panel:${key}` });
        return;
      }
      if (!copy) return problem(node, `<Widget title="${title}"> names neither a catalogue entry (copy) nor a tool panel (panel)`);
      const resolved = resolveCopy(copy);
      if ("problem" in resolved) return problem(node, `<Widget title="${title}">: ${resolved.problem}`);
      widgets.push({ file, line, title, names: resolved.names });
    };

    const visit = (node: ts.Node) => {
      if (ts.isJsxOpeningElement(node) || ts.isJsxSelfClosingElement(node)) {
        const tag = tagName(node);
        if (imports.cards.has(tag) || tag === "Card") problem(node, `renders a Card directly (<${tag}>); render it through Widget`);
        if (imports.widgets.has(tag)) visitWidget(node);
      }
      ts.forEachChild(node, visit);
    };
    visit(sf);
  }

  const views = [...new Set(widgets.map((w) => w.file))].sort();
  return { widgets, views, problems, panelsUsed };
}
