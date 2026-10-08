// "Every web widget" as a source scan (S-617, CR-203 §6, FR-UI-39, FR-UI-40),
// following the S-438 `?raw` precedent (`dashboard/cardEnumeration.test.ts`): no
// view renders a `Card` directly, and every `Widget` names an existing catalogue
// entry or a registered tool panel. The views are DISCOVERED (every non-test
// `.tsx` under src/views/), so a view added later is scanned the day it exists.
import { describe, expect, it } from "vitest";

import { isCopyEntry } from "../copy/text.ts";
import { TOOL_PANELS } from "../copy/toolPanels.ts";
import { scanViewSources, type CatalogueExport, type CatalogueIndex } from "../test/widgetScan.ts";

const sources = import.meta.glob<string>(["/src/views/**/*.tsx", "!/src/views/**/*.test.tsx"], {
  query: "?raw",
  import: "default",
  eager: true,
});

const catalogueModules = import.meta.glob<Record<string, unknown>>("/src/**/*.copy.ts", { eager: true });

/** An object whose values are all catalogue entries (one per dimension, say). */
function isEntryRecord(value: unknown): boolean {
  if (typeof value !== "object" || value === null || isCopyEntry(value)) return false;
  const values = Object.values(value);
  return values.length > 0 && values.every(isCopyEntry);
}

const catalogues: CatalogueIndex = new Map(
  Object.entries(catalogueModules).map(([path, mod]) => [
    path,
    new Map(
      Object.entries(mod).flatMap(([name, value]): [string, CatalogueExport][] =>
        isCopyEntry(value) ? [[name, "entry"]] : isEntryRecord(value) ? [[name, "record"]] : [],
      ),
    ),
  ]),
);

const panels = new Set(Object.keys(TOOL_PANELS));

/** Every view and component source, tests left out: where a widget's markup is written. */
const markupSources = import.meta.glob<string>(
  ["/src/views/**/*.tsx", "/src/components/**/*.tsx", "!/src/**/*.test.tsx"],
  { query: "?raw", import: "default", eager: true },
);

/** The markers of the action line CR-206 removed: its part, its copy, its where chip. */
const ACTION_LINE_MARKER = /data-widget-part=["{]?\W*action|data-widget-copy=["{]?\W*(action|where)\b/;

/** The sources that write an action-line marker, as `file:line`. */
function actionLineMarkers(files: Record<string, string>): string[] {
  return Object.entries(files).flatMap(([file, src]) =>
    src.split("\n").flatMap((line, i) => (ACTION_LINE_MARKER.test(line) ? [`${file}:${i + 1}`] : [])),
  );
}

const scan = (files: Record<string, string>) => scanViewSources(files, catalogues, panels);

describe("widget source scan (S-617)", () => {
  const report = scan(sources);

  it("finds no Card rendered directly, and every Widget names a catalogue entry or a tool panel", () => {
    expect(report.problems.map((p) => `${p.file}:${p.line} ${p.problem}`)).toEqual([]);
    // A finding, not a floor: a widget removed or added later changes this line
    // and fails nothing. Only an empty scan is refused, since it proves nothing.
    expect(report.widgets.length, "the scan read widgets").toBeGreaterThan(0);
    const panelCount = report.widgets.filter((w) => w.names.startsWith("panel:")).length;
    console.info(
      `widget scan: ${report.widgets.length} widgets in ${report.views.length} views ` +
        `(${report.widgets.length - panelCount} figure widgets, ${panelCount} tool panels), ` +
        `over ${Object.keys(sources).length} view source files`,
    );
  });

  it("reads every view source, not a hand-kept list", () => {
    // Each top-level view the registry mounts is a file the glob found.
    for (const file of [
      "/src/views/dashboard/DashboardView.tsx",
      "/src/views/chat/ChatView.tsx",
      "/src/views/workspace/WorkspaceConfigView.tsx",
    ]) {
      expect(Object.keys(sources)).toContain(file);
    }
    expect(Object.keys(sources).some((f) => f.endsWith(".test.tsx")), "tests are not views").toBe(false);
  });

  it("no view or component writes the action line CR-206 removed, and no catalogue entry carries one", () => {
    expect(Object.keys(markupSources)).toContain("/src/components/Widget.tsx");
    expect(actionLineMarkers(markupSources)).toEqual([]);
    const withAction = Object.entries(catalogueModules).flatMap(([path, mod]) =>
      Object.entries(mod).flatMap(([name, value]) => {
        const entries = isCopyEntry(value) ? [[name, value]] : isEntryRecord(value) ? Object.entries(value as object).map(([k, v]) => [`${name}.${k}`, v]) : [];
        return entries.filter(([, entry]) => Object.hasOwn(entry as object, "action")).map(([key]) => `${path}#${key}`);
      }),
    );
    expect(withAction).toEqual([]);
  });

  it("finds an action-line marker written back into a source (falsifiable)", () => {
    const widget = "/src/components/Widget.tsx";
    expect(actionLineMarkers({ [widget]: markupSources[widget] })).toEqual([]);
    for (const marker of [
      '<div data-widget-part="action">',
      '<span data-widget-copy="action">',
      '<p data-widget-copy="where">',
      "<p data-widget-copy={'where'}>",
    ]) {
      expect(actionLineMarkers({ [widget]: `${markupSources[widget]}\n${marker}` }), marker).toEqual([
        `${widget}:${markupSources[widget].split("\n").length + 1}`,
      ]);
    }
    // A near miss is not the marker: a data attribute naming another part, or a word.
    expect(actionLineMarkers({ x: '<div data-widget-part="evidence">actionable where</div>\n<td data-row-action="">' })).toEqual([]);
  });

  it("every registered tool panel is rendered by some view", () => {
    expect([...panels].filter((key) => !report.panelsUsed.has(key))).toEqual([]);
  });

  it("fails on a raw Card added to a real view (the mutation the AC names)", () => {
    const file = "/src/views/gaps/GapsView.tsx";
    const mutated = `${sources[file]}\n\nfunction Extra() {\n  return (\n    <Card title="Extra">\n      <p>raw</p>\n    </Card>\n  );\n}\n`;
    const before = scan({ [file]: sources[file] }).problems;
    const after = scan({ [file]: mutated }).problems;
    expect(after.length, "exactly one problem more than the unmutated view").toBe(before.length + 1);
    expect(after.at(-1)?.problem).toMatch(/renders a Card directly \(<Card>\)/);
  });

  it("fails on a Card imported under another name", () => {
    const src = `import { Card as Panel } from "../../components/index.ts";\nexport const X = () => <Panel title="t">x</Panel>;\n`;
    expect(scan({ "/src/views/x/X.tsx": src }).problems.map((p) => p.problem)).toEqual([
      expect.stringMatching(/renders a Card directly \(<Panel>\)/),
    ]);
  });

  it("reads a Card or Widget imported without an extension, or through a namespace (review fix)", () => {
    const src = `import { Widget } from "../../components/Widget";
import * as C from "../../components/index.ts";
export const A = () => <Widget title="A" />;
export const B = () => <C.Widget title="B" panel="notAPanel" />;
export const D = () => <C.Card title="D">x</C.Card>;
`;
    expect(scan({ "/src/views/x/X.tsx": src }).problems.map((p) => p.problem)).toEqual([
      expect.stringMatching(/<Widget title="A"> names neither/),
      expect.stringMatching(/panel "notAPanel" is not registered/),
      expect.stringMatching(/renders a Card directly \(<C\.Card>\)/),
    ]);
  });

  describe("what a Widget names", () => {
    const head = `import { Widget } from "../../components/index.ts";\n`;
    const file = "/src/views/x/X.tsx";
    const names = (body: string) => {
      const r = scan({ [file]: head + body });
      return { names: r.widgets.map((w) => w.names), problems: r.problems.map((p) => p.problem) };
    };

    it("resolves a direct, aliased, spread, shorthand, record and conditional catalogue entry", () => {
      const src = `import { gate as gateCopy, qualitySignal, DIMENSION_COPY } from "../../copy/health.copy.ts";
export function A() { return <Widget title="Gate" copy={gateCopy} />; }
export function B({ k }: { k: string }) {
  const common = { title: "Q", copy: qualitySignal } as const;
  return <Widget {...common} />;
}
export function C() {
  const copy = DIMENSION_COPY["nesting"];
  const common = { title: "N", copy };
  return <Widget {...common} />;
}
export function D({ a }: { a: boolean }) { return <Widget title="D" copy={a ? gateCopy : qualitySignal} />; }
`;
      expect(names(src)).toEqual({
        names: [
          "/src/copy/health.copy.ts#gate",
          "/src/copy/health.copy.ts#qualitySignal",
          "/src/copy/health.copy.ts#DIMENSION_COPY[…]",
          "/src/copy/health.copy.ts#gate | /src/copy/health.copy.ts#qualitySignal",
        ],
        problems: [],
      });
    });

    it("fails a Widget that names no catalogue entry", () => {
      const local = `const mine = { what: "w", why: "y" };\nexport const A = () => <Widget title="A" copy={mine} />;\n`;
      expect(names(local).problems).toEqual([expect.stringMatching(/copy=\{\{|does not name a catalogue entry/)]);
      expect(names(`export const A = () => <Widget title="A" />;\n`).problems).toEqual([
        expect.stringMatching(/names neither a catalogue entry \(copy\) nor a tool panel \(panel\)/),
      ]);
    });

    it("fails a catalogue import that is not an entry in that module", () => {
      const src = `import { HEALTH_TEXT, missing } from "../../copy/workspaceHealth.copy.ts";
export const A = () => <Widget title="A" copy={missing} />;
export const B = () => <Widget title="B" copy={HEALTH_TEXT} />;
`;
      expect(names(src).problems).toEqual([
        expect.stringMatching(/has no catalogue entry missing/),
        expect.stringMatching(/has no catalogue entry HEALTH_TEXT/),
      ]);
    });

    it("fails a conditional copy when either arm names no catalogue entry (review fix)", () => {
      const src = `import { gate } from "../../copy/health.copy.ts";
const mine = { what: "w", why: "y" };
export const A = ({ c }: { c: boolean }) => <Widget title="A" copy={c ? { ...mine } : gate} />;
export const B = ({ c }: { c: boolean }) => <Widget title="B" copy={c ? gate : { ...mine }} />;
`;
      expect(names(src).problems).toEqual([
        expect.stringMatching(/<Widget title="A">: copy=\{\{ \.\.\.mine \}\} does not name a catalogue entry/),
        expect.stringMatching(/<Widget title="B">: copy=\{\{ \.\.\.mine \}\} does not name a catalogue entry/),
      ]);
    });

    it("fails an indexed copy whose base is not imported, or not a record of entries (review fix)", () => {
      const src = `import { HEALTH_TEXT } from "../../copy/workspaceHealth.copy.ts";
const LOCAL = { x: { what: "w", why: "y" } };
export const A = () => <Widget title="A" copy={LOCAL.x} />;
export const B = ({ k }: { k: "noRules" }) => <Widget title="B" copy={HEALTH_TEXT[k]} />;
`;
      expect(names(src).problems).toEqual([
        expect.stringMatching(/LOCAL is not imported from a catalogue/),
        expect.stringMatching(/HEALTH_TEXT is not a record of catalogue entries/),
      ]);
    });

    it("accepts a registered panel key and fails an unregistered or computed one", () => {
      const src = `export const A = () => <Widget panel="graphQuery" title="Q" />;
export const B = () => <Widget panel="notAPanel" title="N" />;
export const C = ({ k }: { k: "graphQuery" }) => <Widget panel={k} title="C" />;
`;
      const r = names(src);
      expect(r.names).toEqual(["panel:graphQuery"]);
      expect(r.problems).toEqual([
        expect.stringMatching(/panel "notAPanel" is not registered in TOOL_PANELS/),
        expect.stringMatching(/panel is not a literal key/),
      ]);
    });
  });
});
