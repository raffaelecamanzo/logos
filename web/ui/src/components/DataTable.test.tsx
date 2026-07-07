import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it } from "vitest";

import { DataTable, type Column } from "./DataTable.tsx";

afterEach(cleanup);

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
