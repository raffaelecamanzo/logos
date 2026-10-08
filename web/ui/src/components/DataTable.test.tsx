import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import * as ts from "typescript";
import { afterEach, describe, expect, it, vi } from "vitest";

import { DataTable, type Column } from "./DataTable.tsx";

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

interface Row {
  name: string;
  score: number;
}

const COLUMNS: Column<Row>[] = [
  { key: "name", header: "Name", cell: (r) => r.name, sortValue: (r) => r.name },
  { key: "score", header: "Score", numeric: true, cell: (r) => r.score, sortValue: (r) => r.score },
];

// 30 rows in DESCENDING score so the default order is non-trivial.
const ROWS: Row[] = Array.from({ length: 30 }, (_, i) => ({
  name: `row-${String(i).padStart(2, "0")}`,
  score: 30 - i,
}));

function bodyNames(): string[] {
  const rowsEls = screen.getAllByRole("row").slice(1); // drop the header row
  return rowsEls.map((r) => within(r).getAllByRole("cell")[0].textContent ?? "");
}

describe("DataTable pagination (S-188, FR-UI-11)", () => {
  it("shows only the first page and announces the range", () => {
    render(
      <DataTable caption="t" columns={COLUMNS} rows={ROWS} rowKey={(r) => r.name} pageSize={25} />,
    );
    expect(bodyNames()).toHaveLength(25);
    expect(screen.getByRole("status")).toHaveTextContent("Showing 1–25 of 30");
  });

  it("pages forward to the remaining rows with the Next button", async () => {
    const user = userEvent.setup();
    render(
      <DataTable caption="t" columns={COLUMNS} rows={ROWS} rowKey={(r) => r.name} pageSize={25} />,
    );
    await user.click(screen.getByRole("button", { name: "Next page" }));
    expect(bodyNames()).toHaveLength(5);
    expect(screen.getByRole("status")).toHaveTextContent("Showing 26–30 of 30");
  });

  it("sorts the FULL dataset before slicing the page (not just the visible page)", async () => {
    const user = userEvent.setup();
    render(
      <DataTable caption="t" columns={COLUMNS} rows={ROWS} rowKey={(r) => r.name} pageSize={25} />,
    );
    // Sort ascending by score: row-29 (score 1) must lead — proving the sort ran
    // over all 30 rows, not only the 25 on the first page.
    await user.click(screen.getByRole("button", { name: /Score/ }));
    expect(bodyNames()[0]).toBe("row-29");
  });

  it("returns to the first page when the sort changes", async () => {
    const user = userEvent.setup();
    render(
      <DataTable caption="t" columns={COLUMNS} rows={ROWS} rowKey={(r) => r.name} pageSize={25} />,
    );
    await user.click(screen.getByRole("button", { name: "Next page" }));
    expect(screen.getByRole("status")).toHaveTextContent("Showing 26–30 of 30");
    await user.click(screen.getByRole("button", { name: /Name/ }));
    expect(screen.getByRole("status")).toHaveTextContent("Showing 1–25 of 30");
  });

  it("renders no pager when the dataset fits one page", () => {
    render(
      <DataTable
        caption="t"
        columns={COLUMNS}
        rows={ROWS.slice(0, 10)}
        rowKey={(r) => r.name}
        pageSize={25}
      />,
    );
    expect(screen.queryByRole("button", { name: "Next page" })).toBeNull();
  });
});

describe("DataTable header gloss (S-616, FR-UI-39)", () => {
  const GLOSSED: Column<Row>[] = [
    { key: "name", header: "Name", cell: (r) => r.name, sortValue: (r) => r.name },
    { key: "score", header: "Co-change", gloss: "coChange", numeric: true, cell: (r) => r.score, sortValue: (r) => r.score },
  ];

  it("glosses the header through Term, beside — never inside — the sort button", async () => {
    const user = userEvent.setup();
    render(<DataTable caption="t" columns={GLOSSED} rows={ROWS.slice(0, 3)} rowKey={(r) => r.name} />);
    const header = screen.getAllByRole("columnheader")[1];
    const term = header.querySelector("dfn[data-term='coChange']");
    expect(term).not.toBeNull();
    expect(term).toHaveTextContent(/^Co-change/);
    // A focusable <dfn> inside a <button> is invalid nesting, and a click on the
    // term would sort: the term sits outside the button.
    const sort = within(header).getByRole("button", { name: "Co-change" });
    expect(sort.contains(term)).toBe(false);
    // The button's copy of the header is for assistive technology only: seen
    // once, as the term.
    expect(within(sort).getByText("Co-change")).toHaveClass("sr-only");
    // The column is named once, by its header — not "Co-change <definition>
    // Co-change"; the definition stays the term's description.
    expect(screen.getByRole("columnheader", { name: "Co-change" })).toBe(header);
    await user.click(sort);
    expect(header).toHaveAttribute("aria-sort", "ascending");
  });

  it("glosses an unsortable header too", () => {
    const cols: Column<Row>[] = [{ key: "score", header: "Co-change", gloss: "coChange", cell: (r) => r.score }];
    render(<DataTable caption="t" columns={cols} rows={ROWS.slice(0, 1)} rowKey={(r) => r.name} />);
    const header = screen.getByRole("columnheader", { name: "Co-change" });
    expect(header.querySelector("dfn[data-term='coChange']")).not.toBeNull();
    // The header text renders once — as the term — not again beside it.
    const tip = header.querySelector('[role="tooltip"]')!.textContent!;
    expect(header.textContent!.replace(tip, "")).toBe("Co-change ");
  });

  // CR-208 (FR-UI-39, FR-UI-36): a header that is more than its term glosses only
  // the term's words; the term still sits outside the sort button.
  const PARTIAL: Column<Row>[] = [
    {
      key: "score",
      header: "…of which used by another service",
      gloss: "usedByAnotherService",
      glossText: "used by another service",
      numeric: true,
      cell: (r) => r.score,
      sortValue: (r) => r.score,
    },
  ];

  it("glosses only the term's words of a longer header, the rest plain beside it (CR-208)", () => {
    render(<DataTable caption="t" columns={PARTIAL} rows={ROWS.slice(0, 3)} rowKey={(r) => r.name} />);
    const header = screen.getByRole("columnheader", { name: "…of which used by another service" });
    const term = header.querySelector("dfn[data-term='usedByAnotherService']")!;
    expect(term.firstChild?.textContent).toBe("used by another service");
    const sort = within(header).getByRole("button", { name: "…of which used by another service" });
    expect(sort.contains(term)).toBe(false);
    const tip = term.querySelector('[role="tooltip"]')!.textContent!;
    const visible = [...header.childNodes]
      .filter((n) => n !== sort)
      .map((n) => n.textContent)
      .join("")
      .replace(tip, "");
    expect(visible).toBe("…of which used by another service ");
  });

  it("keeps the words after the term too, plain beside it (CR-208)", () => {
    const cols: Column<Row>[] = [
      {
        key: "score",
        header: "Reference resolution (its own)",
        gloss: "referenceResolution",
        glossText: "Reference resolution",
        cell: (r) => r.score,
        sortValue: (r) => r.score,
      },
    ];
    render(<DataTable caption="t" columns={cols} rows={ROWS.slice(0, 1)} rowKey={(r) => r.name} />);
    const header = screen.getByRole("columnheader", { name: "Reference resolution (its own)" });
    const term = header.querySelector("dfn[data-term='referenceResolution']")!;
    expect(term.firstChild?.textContent).toBe("Reference resolution");
    const sort = within(header).getByRole("button");
    const tip = term.querySelector('[role="tooltip"]')!.textContent!;
    const visible = [...header.childNodes]
      .filter((n) => n !== sort)
      .map((n) => n.textContent)
      .join("")
      .replace(tip, "");
    expect(visible).toBe("Reference resolution (its own) ");
  });

  it("has one sort control per glossed header, and activating the term never sorts (CR-208 AC-4)", async () => {
    const user = userEvent.setup();
    render(<DataTable caption="t" columns={[...GLOSSED, ...PARTIAL]} rows={ROWS.slice(0, 3)} rowKey={(r) => r.name} />);
    for (const header of screen.getAllByRole("columnheader").slice(1)) {
      const term = header.querySelector("dfn[data-term]") as HTMLElement;
      const buttons = within(header).getAllByRole("button");
      expect(buttons).toHaveLength(1);
      expect(buttons[0].contains(term)).toBe(false);
      expect(buttons[0].querySelector("dfn, [tabindex]")).toBeNull();
      const before = bodyNames();
      await user.click(term);
      term.focus();
      await user.keyboard("{Enter}");
      await user.keyboard(" ");
      expect(header).toHaveAttribute("aria-sort", "none");
      expect(bodyNames()).toEqual(before);
    }
  });

  it("refuses glossed words the header does not contain, rather than glossing the wrong text", () => {
    const cols: Column<Row>[] = [{ ...PARTIAL[0], glossText: "used by another services" }];
    // React logs the render error it rethrows; the throw is the assertion.
    vi.spyOn(console, "error").mockImplementation(() => {});
    expect(() => render(<DataTable caption="t" columns={cols} rows={ROWS.slice(0, 1)} rowKey={(r) => r.name} />)).toThrow(
      /glossed words "used by another services" are not in the header/,
    );
  });
});

// CR-208 AC-4, "every DataTable": a `Term` written into a column's `header` lands
// inside the sort button (the Members defect). The views' SOURCE is read, as the
// widget scan reads it (`views/widgetScan.test.ts`), so a table added later is held
// the day it exists: a header glosses through the column's `gloss` slot or not at all.
const viewSources = import.meta.glob<string>(["/src/views/**/*.tsx", "!/src/views/**/*.test.tsx"], {
  query: "?raw",
  import: "default",
  eager: true,
});

/** Every `header:` property whose value renders a `Term` element, as file:line. */
function termsInHeaders(file: string, source: string): string[] {
  const sf = ts.createSourceFile(file, source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  const found: string[] = [];
  const rendersTerm = (node: ts.Node): boolean => {
    if (
      (ts.isJsxOpeningElement(node) || ts.isJsxSelfClosingElement(node)) &&
      node.tagName.getText(sf) === "Term"
    ) {
      return true;
    }
    return ts.forEachChild(node, (child) => (rendersTerm(child) ? true : undefined)) ?? false;
  };
  const visit = (node: ts.Node) => {
    if (ts.isPropertyAssignment(node) && node.name.getText(sf) === "header" && rendersTerm(node.initializer)) {
      found.push(`${file}:${sf.getLineAndCharacterOfPosition(node.getStart(sf)).line + 1}`);
    }
    ts.forEachChild(node, visit);
  };
  visit(sf);
  return found;
}

describe("no column header renders a Term inside its sort button (CR-208 AC-4)", () => {
  it("reads the view sources (a finding, not a floor)", () => {
    expect(Object.keys(viewSources).length).toBeGreaterThan(0);
    console.info(`DataTable header scan: ${Object.keys(viewSources).length} view sources`);
  });

  it("finds a Term written into a header — the shape the Members table had", () => {
    const probe = `const C = [{ key: "a", header: (<>…of which <Term term="usedByAnotherService" /></>), cell: () => 1 }];`;
    expect(termsInHeaders("probe.tsx", probe)).toEqual(["probe.tsx:1"]);
    // Its near miss: a Term in a cell, or a glossed string header, is not one.
    const clean = `const C = [{ key: "a", header: "Calls", gloss: "arm", cell: () => <Term term="arm" /> }];`;
    expect(termsInHeaders("clean.tsx", clean)).toEqual([]);
  });

  it("finds none in any view", () => {
    expect(Object.entries(viewSources).flatMap(([file, source]) => termsInHeaders(file, source))).toEqual([]);
  });
});
